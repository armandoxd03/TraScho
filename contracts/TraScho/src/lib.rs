#![no_std]

use soroban_sdk::{
    contract, contractimpl, contracttype, token, Address, Env, Map, String, Symbol, Vec,
};

// ---------------------------------------------------------------------------
// Storage key namespaces
// ---------------------------------------------------------------------------

/// Top-level keys stored in persistent contract storage.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// Admin address — the scholarship office wallet.
    Admin,
    /// Token contract address used for disbursements (USDC on Stellar).
    Token,
    /// Per-student eligibility record, keyed by student Address.
    Student(Address),
    /// Running list of all registered student addresses (for iteration).
    StudentList,
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Eligibility criteria that must ALL be satisfied before a payout fires.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct EligibilityRecord {
    /// Student's full name (stored on-chain for transparency).
    pub name: String,
    /// Current grade (0–100 scale). Minimum 75 to qualify.
    pub grade: u32,
    /// Attendance percentage (0–100). Minimum 80 to qualify.
    pub attendance: u32,
    /// Monthly allowance amount in stroops (1 XLM = 10_000_000 stroops).
    pub allowance_amount: i128,
    /// Whether the student has already received this month's disbursement.
    pub disbursed_this_period: bool,
    /// Total lifetime amount disbursed to this student (audit trail).
    pub total_disbursed: i128,
}

// ---------------------------------------------------------------------------
// Contract
// ---------------------------------------------------------------------------

#[contract]
pub struct TraSchoContract;

#[contractimpl]
impl TraSchoContract {
    // -----------------------------------------------------------------------
    // Initialization
    // -----------------------------------------------------------------------

    /// Initialize the contract once. Sets the admin (scholarship office)
    /// and the token to be disbursed (USDC or custom scholarship token).
    pub fn initialize(env: Env, admin: Address, token: Address) {
        // Prevent re-initialization: panic if admin already set.
        if env.storage().persistent().has(&DataKey::Admin) {
            panic!("already initialized");
        }

        // Require the deployer to authorize this call.
        admin.require_auth();

        env.storage().persistent().set(&DataKey::Admin, &admin);
        env.storage().persistent().set(&DataKey::Token, &token);

        // Initialize an empty student list.
        let empty: Vec<Address> = Vec::new(&env);
        env.storage().persistent().set(&DataKey::StudentList, &empty);
    }

    // -----------------------------------------------------------------------
    // Admin helpers
    // -----------------------------------------------------------------------

    /// Returns the current admin address.
    pub fn get_admin(env: Env) -> Address {
        env.storage().persistent().get(&DataKey::Admin).unwrap()
    }

    /// Returns the token contract address used for disbursements.
    pub fn get_token(env: Env) -> Address {
        env.storage().persistent().get(&DataKey::Token).unwrap()
    }

    // -----------------------------------------------------------------------
    // Student registration
    // -----------------------------------------------------------------------

    /// Admin registers a student with their initial record.
    /// `allowance_amount` is in the token's smallest unit (e.g., stroops or
    /// USDC cents depending on the asset's decimal config).
    pub fn register_student(
        env: Env,
        student: Address,
        name: String,
        allowance_amount: i128,
    ) {
        // Only the scholarship-office admin may register students.
        let admin: Address = env.storage().persistent().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        // Reject duplicate registrations.
        if env.storage().persistent().has(&DataKey::Student(student.clone())) {
            panic!("student already registered");
        }

        let record = EligibilityRecord {
            name,
            grade: 0,
            attendance: 0,
            allowance_amount,
            disbursed_this_period: false,
            total_disbursed: 0,
        };

        env.storage()
            .persistent()
            .set(&DataKey::Student(student.clone()), &record);

        // Append student to the master list.
        let mut list: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::StudentList)
            .unwrap_or_else(|| Vec::new(&env));
        list.push_back(student);
        env.storage().persistent().set(&DataKey::StudentList, &list);
    }

    // -----------------------------------------------------------------------
    // Eligibility updates
    // -----------------------------------------------------------------------

    /// Admin updates a student's grade and attendance for the current period.
    /// This is called after the registrar system syncs records on-chain.
    pub fn update_eligibility(
        env: Env,
        student: Address,
        grade: u32,
        attendance: u32,
    ) {
        // Only admin may push verified academic data.
        let admin: Address = env.storage().persistent().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let mut record: EligibilityRecord = env
            .storage()
            .persistent()
            .get(&DataKey::Student(student.clone()))
            .expect("student not found");

        record.grade = grade;
        record.attendance = attendance;
        // Reset the disbursement flag so the student can be paid this period.
        record.disbursed_this_period = false;

        env.storage()
            .persistent()
            .set(&DataKey::Student(student), &record);
    }

    // -----------------------------------------------------------------------
    // Core MVP: disburse_allowance
    // -----------------------------------------------------------------------

    /// Disburse monthly allowance to a student **if** they meet eligibility
    /// thresholds: grade ≥ 75 and attendance ≥ 80.
    ///
    /// Transaction flow (MVP demo path):
    ///   1. Student (or admin on behalf) calls this function.
    ///   2. Contract reads the student's eligibility record from storage.
    ///   3. Contract checks grade ≥ 75 AND attendance ≥ 80.
    ///   4. If eligible and not yet disbursed this period, contract calls the
    ///      Stellar token contract to transfer `allowance_amount` from the
    ///      contract's own balance to the student's wallet — in one atomic tx.
    ///   5. The record is updated: `disbursed_this_period = true` and
    ///      `total_disbursed` increments — creating an immutable audit trail.
    pub fn disburse_allowance(env: Env, student: Address) {
        // The student must authorize the claim (prevents griefing).
        student.require_auth();

        let mut record: EligibilityRecord = env
            .storage()
            .persistent()
            .get(&DataKey::Student(student.clone()))
            .expect("student not registered");

        // --- Eligibility gate ---
        if record.grade < 75 {
            panic!("grade below minimum threshold (75)");
        }
        if record.attendance < 80 {
            panic!("attendance below minimum threshold (80)");
        }
        if record.disbursed_this_period {
            panic!("allowance already disbursed this period");
        }

        // --- On-chain token transfer ---
        let token_id: Address = env.storage().persistent().get(&DataKey::Token).unwrap();
        let token_client = token::Client::new(&env, &token_id);

        // The contract itself holds the scholarship pool funds.
        // It transfers `allowance_amount` to the student's address.
        token_client.transfer(
            &env.current_contract_address(),
            &student,
            &record.allowance_amount,
        );

        // --- Update audit trail ---
        record.disbursed_this_period = true;
        record.total_disbursed += record.allowance_amount;

        env.storage()
            .persistent()
            .set(&DataKey::Student(student.clone()), &record);

        // Emit a diagnostic event for off-chain indexers / dashboards.
        env.events().publish(
            (Symbol::new(&env, "disbursed"),),
            (student, record.allowance_amount, record.total_disbursed),
        );
    }

    // -----------------------------------------------------------------------
    // Batch disburse (bonus: admin triggers all eligible students at once)
    // -----------------------------------------------------------------------

    /// Admin calls this to sweep through ALL registered students and disburse
    /// to every eligible, not-yet-paid student in a single transaction.
    pub fn batch_disburse(env: Env) {
        let admin: Address = env.storage().persistent().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let token_id: Address = env.storage().persistent().get(&DataKey::Token).unwrap();
        let token_client = token::Client::new(&env, &token_id);

        let list: Vec<Address> = env
            .storage()
            .persistent()
            .get(&DataKey::StudentList)
            .unwrap_or_else(|| Vec::new(&env));

        for student in list.iter() {
            let key = DataKey::Student(student.clone());
            if let Some(mut record) = env.storage().persistent().get::<DataKey, EligibilityRecord>(&key) {
                if record.grade >= 75
                    && record.attendance >= 80
                    && !record.disbursed_this_period
                {
                    token_client.transfer(
                        &env.current_contract_address(),
                        &student,
                        &record.allowance_amount,
                    );
                    record.disbursed_this_period = true;
                    record.total_disbursed += record.allowance_amount;
                    env.storage().persistent().set(&key, &record);

                    env.events().publish(
                        (Symbol::new(&env, "batch_disbursed"),),
                        (student, record.allowance_amount),
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Read helpers (for frontend / auditors)
    // -----------------------------------------------------------------------

    /// Returns the full eligibility record for a student.
    pub fn get_student(env: Env, student: Address) -> EligibilityRecord {
        env.storage()
            .persistent()
            .get(&DataKey::Student(student))
            .expect("student not found")
    }

    /// Returns the list of all registered student addresses.
    pub fn get_student_list(env: Env) -> Vec<Address> {
        env.storage()
            .persistent()
            .get(&DataKey::StudentList)
            .unwrap_or_else(|| Vec::new(&env))
    }
}
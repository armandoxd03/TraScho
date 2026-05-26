#[cfg(test)]
mod tests {
    use soroban_sdk::{
        testutils::{Address as _, AuthorizedFunction, AuthorizedInvocation},
        token, Address, Env, IntoVal, String,
    };

    use crate::{TraSchoContract, TraSchoContractClient};

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Deploys the TraScho contract + a mock USDC token, registers one student,
    /// and funds the contract with enough tokens to disburse.
    fn setup() -> (Env, Address, Address, Address, Address, i128) {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let student = Address::generate(&env);
        let allowance: i128 = 5_000_000; // 5 USDC (6 decimals)

        // Deploy a Stellar-standard token (USDC mock).
        let token_id = env.register_stellar_asset_contract(admin.clone());
        let token_admin_client = token::StellarAssetClient::new(&env, &token_id);

        // Deploy the TraScho contract.
        let contract_id = env.register_contract(None, TraSchoContract);
        let client = TraSchoContractClient::new(&env, &contract_id);

        // Initialize contract.
        client.initialize(&admin, &token_id);

        // Fund the contract's pool (simulates the scholarship office depositing funds).
        token_admin_client.mint(&contract_id, &(allowance * 10));

        // Register one student.
        client.register_student(
            &student,
            &String::from_str(&env, "Maria Santos"),
            &allowance,
        );

        (env, contract_id, token_id, admin, student, allowance)
    }

    // -----------------------------------------------------------------------
    // Test 1 — Happy path: eligible student receives allowance end-to-end
    // -----------------------------------------------------------------------
    #[test]
    fn test_happy_path_disburse_allowance() {
        let (env, contract_id, token_id, admin, student, allowance) = setup();
        let client = TraSchoContractClient::new(&env, &contract_id);
        let token_client = token::Client::new(&env, &token_id);

        // Admin pushes verified academic data: grade 88, attendance 92.
        client.update_eligibility(&student, &88, &92);

        // Record balance before.
        let balance_before = token_client.balance(&student);

        // Student claims their allowance.
        client.disburse_allowance(&student);

        // Assert the student's wallet received the correct amount.
        let balance_after = token_client.balance(&student);
        assert_eq!(
            balance_after - balance_before,
            allowance,
            "student should have received exactly the allowance amount"
        );

        // Assert the disbursement flag is now true.
        let record = client.get_student(&student);
        assert!(record.disbursed_this_period, "disbursed_this_period must be true");
        assert_eq!(record.total_disbursed, allowance, "total_disbursed must equal one allowance");
    }

    // -----------------------------------------------------------------------
    // Test 2 — Edge case: student below grade threshold cannot claim
    // -----------------------------------------------------------------------
    #[test]
    #[should_panic(expected = "grade below minimum threshold (75)")]
    fn test_edge_case_low_grade_rejected() {
        let (env, contract_id, _token_id, _admin, student, _allowance) = setup();
        let client = TraSchoContractClient::new(&env, &contract_id);

        // Admin sets a failing grade (74) with good attendance.
        client.update_eligibility(&student, &74, &90);

        // This must panic — student does not qualify.
        client.disburse_allowance(&student);
    }

    // -----------------------------------------------------------------------
    // Test 3 — State verification: storage reflects correct state after disburse
    // -----------------------------------------------------------------------
    #[test]
    fn test_state_verification_after_disburse() {
        let (env, contract_id, _token_id, _admin, student, allowance) = setup();
        let client = TraSchoContractClient::new(&env, &contract_id);

        client.update_eligibility(&student, &90, &95);
        client.disburse_allowance(&student);

        let record = client.get_student(&student);

        // disbursed_this_period flag must flip to true.
        assert!(record.disbursed_this_period);

        // Cumulative amount must be exactly one allowance cycle.
        assert_eq!(record.total_disbursed, allowance);

        // Grade and attendance remain as set by admin (not wiped on disburse).
        assert_eq!(record.grade, 90);
        assert_eq!(record.attendance, 95);
    }

    // -----------------------------------------------------------------------
    // Test 4 — Edge case: double-claim in same period is rejected
    // -----------------------------------------------------------------------
    #[test]
    #[should_panic(expected = "allowance already disbursed this period")]
    fn test_double_claim_rejected() {
        let (env, contract_id, _token_id, _admin, student, _allowance) = setup();
        let client = TraSchoContractClient::new(&env, &contract_id);

        client.update_eligibility(&student, &85, &88);
        client.disburse_allowance(&student); // First claim — OK.
        client.disburse_allowance(&student); // Second claim — must panic.
    }

    // -----------------------------------------------------------------------
    // Test 5 — Edge case: unregistered student cannot claim
    // -----------------------------------------------------------------------
    #[test]
    #[should_panic(expected = "student not registered")]
    fn test_unregistered_student_cannot_claim() {
        let (env, contract_id, _token_id, _admin, _student, _allowance) = setup();
        let client = TraSchoContractClient::new(&env, &contract_id);

        // Generate a random address that was never registered.
        let stranger = Address::generate(&env);
        client.disburse_allowance(&stranger);
    }
}
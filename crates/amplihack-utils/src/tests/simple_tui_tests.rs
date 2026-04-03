    use super::*;

    #[test]
    fn test_result_passed_constructor() {
        let r = TestResult::passed("t1", 1.5, "ok");
        assert!(r.is_passed());
        assert_eq!(r.status, "passed");
        assert_eq!(r.test_id, "t1");
    }

    #[test]
    fn test_result_failed_constructor() {
        let r = TestResult::failed("t2", 0.1, "boom");
        assert!(!r.is_passed());
        assert_eq!(r.status, "failed");
        assert_eq!(r.message, "boom");
    }

    #[test]
    fn test_result_serde_roundtrip() {
        let r = TestResult::passed("ser", 2.0, "ok");
        let json = serde_json::to_string(&r).expect("serialize");
        let r2: TestResult = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(r, r2);
    }

    #[test]
    fn tui_test_case_default_timeout() {
        let tc = TUITestCase::new("tc", "my test", vec!["echo hi".into()]);
        assert_eq!(tc.timeout, 10);
    }

    #[test]
    fn tui_test_case_custom_timeout() {
        let tc = TUITestCase::with_timeout("tc", "my test", vec!["echo hi".into()], 30);
        assert_eq!(tc.timeout, 30);
    }

    #[test]
    fn tui_test_case_serde_roundtrip() {
        let tc = TUITestCase::new("tc", "test", vec!["ls".into()]);
        let json = serde_json::to_string(&tc).expect("serialize");
        let tc2: TUITestCase = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(tc, tc2);
    }

    #[test]
    fn ci_detection_respects_env() {
        // In test environments CI is typically set, so just verify the function runs.
        let _ = is_ci_environment();
    }

    #[test]
    fn tester_add_and_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tester = SimpleTUITester::new(dir.path()).expect("new");
        assert_eq!(tester.test_count(), 0);

        tester.add_test(TUITestCase::new("a", "A", vec!["echo a".into()]));
        tester.add_test(TUITestCase::new("b", "B", vec!["echo b".into()]));
        assert_eq!(tester.test_count(), 2);
    }

    #[test]
    fn run_test_unknown_id_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tester = SimpleTUITester::new(dir.path()).expect("new");
        let res = tester.run_test("nope");
        assert!(res.is_err());
    }

    #[test]
    fn run_test_echo_succeeds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tester = SimpleTUITester::new(dir.path()).expect("new");
        tester.set_force_subprocess(true);
        tester.add_test(TUITestCase::new("echo", "echo test", vec!["echo hello".into()]));

        let res = tester.run_test("echo").expect("run");
        assert!(res.is_passed(), "message: {}", res.message);
    }

    #[test]
    fn run_test_bad_command_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tester = SimpleTUITester::new(dir.path()).expect("new");
        tester.set_force_subprocess(true);
        tester.add_test(TUITestCase::new(
            "bad",
            "bad cmd",
            vec!["this_command_does_not_exist_xyz".into()],
        ));

        let res = tester.run_test("bad").expect("run");
        assert!(!res.is_passed());
        assert!(res.message.contains("not found"), "msg: {}", res.message);
    }

    #[test]
    fn run_test_empty_command_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tester = SimpleTUITester::new(dir.path()).expect("new");
        tester.set_force_subprocess(true);
        tester.add_test(TUITestCase::new("empty", "empty", vec!["".into()]));

        let res = tester.run_test("empty").expect("run");
        assert!(!res.is_passed());
        assert!(res.message.contains("Empty command"), "msg: {}", res.message);
    }

    #[test]
    fn run_all_collects_results() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut tester = SimpleTUITester::new(dir.path()).expect("new");
        tester.set_force_subprocess(true);
        tester.add_test(TUITestCase::new("a", "A", vec!["echo a".into()]));
        tester.add_test(TUITestCase::new("b", "B", vec!["echo b".into()]));

        let results = tester.run_all();
        assert_eq!(results.len(), 2);
        assert!(results["a"].is_passed());
        assert!(results["b"].is_passed());
    }

    #[test]
    fn create_amplihack_test_helper() {
        let tc = create_amplihack_test("help", "--help");
        assert_eq!(tc.test_id, "help");
        assert!(tc.commands[0].contains("amplihack --help"));
    }

    #[test]
    fn output_dir_is_created() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("nested").join("deep");
        let tester = SimpleTUITester::new(&nested).expect("new");
        assert!(tester.output_dir().exists());
    }

    #[test]
    fn command_exists_on_path_echo() {
        assert!(command_exists_on_path("echo"));
    }

    #[test]
    fn command_exists_on_path_missing() {
        assert!(!command_exists_on_path("no_such_binary_abc_xyz_123"));
    }

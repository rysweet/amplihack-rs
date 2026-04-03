    use super::*;

    // -- Stub implementations -----------------------------------------------

    struct StubQuestionGen {
        questions: Vec<Question>,
    }

    impl QuestionGenerator for StubQuestionGen {
        fn generate_all_questions(&self, _topic: &str) -> Result<Vec<Question>, KnowledgeBuilderError> {
            Ok(self.questions.clone())
        }
    }

    struct StubKnowledgeAcq;

    impl KnowledgeAcquirer for StubKnowledgeAcq {
        fn answer_all_questions(
            &self,
            mut questions: Vec<Question>,
            _topic: &str,
        ) -> Result<Vec<Question>, KnowledgeBuilderError> {
            for q in &mut questions {
                q.answer = format!("Answer to: {}", q.text);
            }
            Ok(questions)
        }
    }

    struct StubArtifactGen;

    impl ArtifactGenerator for StubArtifactGen {
        fn generate_all(
            &self,
            _graph: &KnowledgeGraph,
        ) -> Result<Vec<PathBuf>, KnowledgeBuilderError> {
            Ok(vec![PathBuf::from("summary.md")])
        }
    }

    struct FailingQuestionGen;

    impl QuestionGenerator for FailingQuestionGen {
        fn generate_all_questions(&self, _topic: &str) -> Result<Vec<Question>, KnowledgeBuilderError> {
            Err(KnowledgeBuilderError::PipelineStep("question gen failed".into()))
        }
    }

    struct FailingArtifactGen;

    impl ArtifactGenerator for FailingArtifactGen {
        fn generate_all(
            &self,
            _graph: &KnowledgeGraph,
        ) -> Result<Vec<PathBuf>, KnowledgeBuilderError> {
            Err(KnowledgeBuilderError::PipelineStep("artifact gen failed".into()))
        }
    }

    // -- Helpers ------------------------------------------------------------

    fn sample_questions() -> Vec<Question> {
        vec![
            Question { text: "What is Rust?".into(), depth: 0, parent_index: None, answer: String::new() },
            Question { text: "Why memory safety?".into(), depth: 1, parent_index: Some(0), answer: String::new() },
        ]
    }

    fn make_builder(
        topic: &str,
    ) -> KnowledgeBuilder<StubQuestionGen, StubKnowledgeAcq, StubArtifactGen> {
        let config = KnowledgeBuilderConfig::new(topic, Some("test-agent".into()), Some(PathBuf::from("/fake/output")));
        KnowledgeBuilder::new(config, StubQuestionGen { questions: sample_questions() }, StubKnowledgeAcq, StubArtifactGen)
    }

    // -- Tests --------------------------------------------------------------

    #[test]
    fn topic_slug_basic() {
        assert_eq!(topic_slug("Hello World"), "hello_world");
    }

    #[test]
    fn topic_slug_special_chars() {
        assert_eq!(topic_slug("Rust & C++"), "rust___c__");
    }

    #[test]
    fn topic_slug_truncates_at_50() {
        let long = "a".repeat(100);
        assert_eq!(topic_slug(&long).len(), 50);
    }

    #[test]
    fn topic_slug_empty() {
        assert_eq!(topic_slug(""), "");
    }

    #[test]
    fn config_defaults() {
        let cfg = KnowledgeBuilderConfig::new("Rust", None, None);
        assert_eq!(cfg.topic, "Rust");
        assert!(!cfg.agent_cmd.is_empty());
        assert!(cfg.output_dir().to_string_lossy().contains("rust"));
    }

    #[test]
    fn config_custom_output() {
        let cfg = KnowledgeBuilderConfig::new("AI Safety", Some("my-agent".into()), Some(PathBuf::from("/out")));
        assert_eq!(cfg.agent_cmd, "my-agent");
        assert_eq!(cfg.output_dir(), PathBuf::from("/out/ai_safety"));
    }

    #[test]
    fn config_trims_topic() {
        let cfg = KnowledgeBuilderConfig::new("  padded  ", None, None);
        assert_eq!(cfg.topic, "padded");
    }

    #[test]
    fn builder_accessors() {
        let b = make_builder("Rust");
        assert_eq!(b.topic(), "Rust");
        assert_eq!(b.agent_cmd(), "test-agent");
        assert!(b.output_dir().to_string_lossy().contains("rust"));
    }

    #[test]
    fn build_success_returns_output_dir() {
        let b = make_builder("Rust programming");
        let result = b.build();
        assert!(result.is_ok());
        let dir = result.expect("should succeed");
        assert!(dir.to_string_lossy().contains("rust_programming"));
    }

    #[test]
    fn build_populates_answers() {
        // Build internally uses the stubs which populate answers; verify
        // the pipeline ran by checking the return path.
        let b = make_builder("test topic");
        assert!(b.build().is_ok());
    }

    #[test]
    fn build_question_gen_failure() {
        let config = KnowledgeBuilderConfig::new("fail", Some("x".into()), Some(PathBuf::from("/x")));
        let b = KnowledgeBuilder::new(config, FailingQuestionGen, StubKnowledgeAcq, StubArtifactGen);
        let res = b.build();
        assert!(res.is_err());
        let msg = res.unwrap_err().to_string();
        assert!(msg.contains("question gen failed"), "msg: {msg}");
    }

    #[test]
    fn build_artifact_gen_failure() {
        let config = KnowledgeBuilderConfig::new("fail", Some("x".into()), Some(PathBuf::from("/x")));
        let b = KnowledgeBuilder::new(
            config,
            StubQuestionGen { questions: sample_questions() },
            StubKnowledgeAcq,
            FailingArtifactGen,
        );
        let res = b.build();
        assert!(res.is_err());
        let msg = res.unwrap_err().to_string();
        assert!(msg.contains("artifact gen failed"), "msg: {msg}");
    }

    #[test]
    fn create_config_helper() {
        let cfg = create_config("my topic", Some("agent".into()), Some(Path::new("/base")));
        assert_eq!(cfg.topic, "my topic");
        assert_eq!(cfg.agent_cmd, "agent");
        assert_eq!(cfg.output_dir(), PathBuf::from("/base/my_topic"));
    }

    #[test]
    fn error_display_pipeline() {
        let e = KnowledgeBuilderError::PipelineStep("boom".into());
        assert_eq!(e.to_string(), "pipeline step failed: boom");
    }

    #[test]
    fn error_display_io() {
        let e = KnowledgeBuilderError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
        assert!(e.to_string().contains("gone"));
    }

pub mod engine;
pub mod interaction;
pub(crate) mod llm;
pub mod prompt;
pub(crate) mod sandbox;
pub mod suggestions;
pub mod tools;

pub use engine::CodeModeEngine;

#[cfg(test)]
mod interaction_trait_test {
    use crate::interaction::UserInteraction;
    use std::sync::Arc;

    struct Echo;
    impl UserInteraction for Echo {
        fn ask(&self, _question: &str, _choices: &[&str]) -> String {
            "echo".to_string()
        }
    }

    #[test]
    fn test_trait_object_works() {
        let i: Arc<dyn UserInteraction> = Arc::new(Echo);
        assert_eq!(i.ask("q?", &["a", "b"]), "echo");
    }
}

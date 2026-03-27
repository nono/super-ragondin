/// Backend for user interaction during codemode execution.
///
/// Implement this trait to provide a concrete I/O mechanism (CLI stdin,
/// Tauri event, etc.). The trait is `Send + Sync` so it can be stored
/// inside `Arc` and shared across threads.
pub trait UserInteraction: Send + Sync {
    /// Ask the user a clarifying question with 2–3 labelled choices.
    ///
    /// The user may pick a numbered choice (1-based) or type a free-form
    /// answer. Returns the user's response as a plain string.
    fn ask(&self, question: &str, choices: &[&str]) -> String;
}

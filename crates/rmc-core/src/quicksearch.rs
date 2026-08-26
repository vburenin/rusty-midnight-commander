#[derive(Debug, Clone, Default)]
pub struct QuickSearchState {
    pub pattern: String,
    /// Index to continue \"next\" search from (exclusive). Uses current cursor if None.
    pub next_from: Option<usize>,
}

impl QuickSearchState {
    pub fn new() -> Self {
        Self {
            pattern: String::new(),
            next_from: None,
        }
    }
}

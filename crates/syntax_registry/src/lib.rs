mod grammar;
mod highlight;
mod loader;
mod registry;

pub use grammar::Grammar;
pub use highlight::{HighlightCategory, HighlightSpan};
pub use registry::{Handle, PollResult, SyntaxRegistry};

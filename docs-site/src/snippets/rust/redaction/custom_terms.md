```rust title="Rust"
use xberg::{
    ExtractionConfig, RedactionConfig, RedactionStrategy, RedactionTerm, RedactionPattern,
};

fn main() {
    let config = ExtractionConfig {
        redaction: Some(RedactionConfig {
            strategy: RedactionStrategy::TokenReplace,
            custom_terms: vec![
                RedactionTerm::labeled("Project", "Project Polaris"),
                RedactionTerm { label: "Employee".into(), value: "EMP-7421".into(), case_sensitive: true },
            ],
            custom_patterns: vec![
                RedactionPattern::labeled("InternalId", r"INT-\d{6}"),
            ],
            ..Default::default()
        }),
        ..Default::default()
    };
}
```

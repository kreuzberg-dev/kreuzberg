<!-- snippet:syntax-only reason="TOML validates at syntax level only" -->
```toml title="xberg.toml"
[redaction]
categories = ["email", "phone", "ssn", "credit_card", "iban"]
strategy = "mask"

[[redaction.custom_terms]]
label = "Project"
value = "Project Polaris"

[[redaction.custom_patterns]]
label = "InternalId"
pattern = "INT-\\d{6}"
```

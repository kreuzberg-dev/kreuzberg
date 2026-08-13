<!-- snippet:syntax-only reason="TOML validates at syntax level only" -->
```toml title="xberg.toml"
[ner]
backend = "llm"
custom_labels = ["Treatment", "Vessel", "Product"]

[ner.llm]
model = "openai/gpt-4o-mini"
```

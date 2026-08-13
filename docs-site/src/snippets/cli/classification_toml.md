<!-- snippet:syntax-only reason="TOML validates at syntax level only" -->
```toml title="xberg.toml"
[page_classification]
labels = ["invoice", "contract", "id_document", "receipt"]
multi_label = false

[page_classification.llm]
model = "openai/gpt-4o-mini"
```

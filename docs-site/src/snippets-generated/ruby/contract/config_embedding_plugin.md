---
id: fixture_ruby_config_embedding_plugin
language: ruby
target: ruby
level: typecheck
requires: []
side_effect: server
---

Tests EmbeddingModelType::Plugin variant deserialization in ChunkingConfig — config accepts the plugin variant shape; actual dispatch requires a host-language backend registered via register_embedding_backend at runtime

```ruby title="Ruby"
require "xberg"
result = Xberg.extract(ExtractInput.new(kind: 'uri', uri: 'https://example.com/pdf/fake_memo.pdf'), { 'chunking' => { 'embedding' => { 'max_embed_duration_secs' => 30, 'model' => { 'name' => 'test-plugin-backend', 'type' => 'plugin' }, 'normalize' => true }, 'max_chars' => 500, 'max_overlap' => 50 } })
puts result.inspect

```

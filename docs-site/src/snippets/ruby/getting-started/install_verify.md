```ruby title="Ruby"
require 'xberg'

puts "Xberg version: #{Xberg::VERSION}"
puts "FFI bindings loaded successfully"

input = Xberg::ExtractInput.new(uri: 'sample.pdf')
config = Xberg::ExtractionConfig.new
result = Xberg.extract(input, config)
puts "Installation verified! Extracted #{result.results.first.content.length} characters"
```

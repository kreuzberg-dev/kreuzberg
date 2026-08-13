```ruby title="Ruby"
require 'xberg'

input = Xberg::ExtractInput.new(uri: 'document.pdf')
config = Xberg::ExtractionConfig.new
result = Xberg.extract(input, config)

puts "Content:"
puts result.results.first.content

puts "\nMetadata:"
puts "Title: #{result.results.first.metadata&.dig('title')}"
puts "Author: #{result.results.first.metadata&.dig('author')}"

puts "\nTables found: #{result.results.first.tables.length}"
puts "Images found: #{result.results.first.images.length}"
```

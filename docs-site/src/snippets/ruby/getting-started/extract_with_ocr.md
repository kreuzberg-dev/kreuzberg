```ruby title="Ruby"
require 'xberg'

ocr_config = Xberg::OcrConfig.new(
  backend: 'tesseract',
  language: 'eng'
)

config = Xberg::ExtractionConfig.new(ocr: ocr_config)
input = Xberg::ExtractInput.new(uri: 'scanned.pdf')
result = Xberg.extract(input, config)

puts "Extracted text from scanned document:"
puts result.results.first.content
puts "Used OCR backend: tesseract"
```

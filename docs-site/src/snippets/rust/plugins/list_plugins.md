```rust title="Rust"
use xberg::{list_document_extractors, list_post_processors, list_ocr_backends, list_validators};

let extractors = list_document_extractors()?;
println!("Registered extractors: {:?}", extractors);

let processors = list_post_processors()?;
println!("Registered processors: {:?}", processors);

let backends = list_ocr_backends()?;
println!("Registered OCR backends: {:?}", backends);

let validators = list_validators()?;
println!("Registered validators: {:?}", validators);
```

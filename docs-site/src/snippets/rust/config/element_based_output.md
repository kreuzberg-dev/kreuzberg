```rust title="Element-Based Output (Rust)"
use xberg::{extract, ExtractionConfig, ExtractInput};
use xberg::ResultFormat;

#[tokio::main]
async fn main() -> xberg::Result<()> {
    // Configure element-based output (result_format controls Unified vs ElementBased)
    let config = ExtractionConfig {
        result_format: ResultFormat::ElementBased,
        ..Default::default()
    };

    // Extract document
    let output = extract(ExtractInput::from_uri("document.pdf"), &config).await?;
    let result = &output.results[0];

    // Access elements
    if let Some(elements) = &result.elements {
        for element in elements {
            println!("Type: {:?}", element.element_type);
            println!("Text: {}", &element.text[..100.min(element.text.len())]);

            if let Some(page) = element.metadata.page_number {
                println!("Page: {}", page);
            }

            if let Some(coords) = &element.metadata.coordinates {
                println!("Coords: ({}, {}) - ({}, {})",
                    coords.x0, coords.y0, coords.x1, coords.y1);
            }

            println!("---");
        }

        // Filter by element type
        let titles: Vec<_> = elements.iter()
            .filter(|e| matches!(e.element_type, xberg::types::ElementType::Title))
            .collect();

        for title in titles {
            let level = title.metadata.additional.get("level")
                .map(|v| v.as_ref())
                .unwrap_or("unknown");
            println!("[{}] {}", level, title.text);
        }
    }

    Ok(())
}
```

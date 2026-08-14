```rust title="Rust"
use xberg::{extract, ExtractionConfig, ExtractInput, FormatMetadata};

#[tokio::main]
async fn main() -> xberg::Result<()> {
    let output = extract(ExtractInput::from_uri("document.pdf"), &ExtractionConfig::default()).await?;
    let result = &output.results[0];

    // Common bibliographic fields live on `Metadata` directly.
    if let Some(title) = &result.metadata.title {
        println!("Title: {}", title);
    }
    if let Some(authors) = &result.metadata.authors {
        println!("Author: {}", authors.join(", "));
    }

    // Format-specific fields are behind the `FormatMetadata` discriminated union.
    if let Some(FormatMetadata::Pdf(pdf_meta)) = &result.metadata.format {
        if let Some(pages) = pdf_meta.page_count {
            println!("Pages: {}", pages);
        }
    }

    let html_output = extract(ExtractInput::from_uri("page.html"), &ExtractionConfig::default()).await?;
    let html_result = &html_output.results[0];
    if let Some(FormatMetadata::Html(html_meta)) = &html_result.metadata.format {
        if let Some(title) = &html_meta.title {
            println!("Title: {}", title);
        }
        if let Some(desc) = &html_meta.description {
            println!("Description: {}", desc);
        }

        // Access keywords array
        println!("Keywords: {:?}", html_meta.keywords);

        // Access canonical URL (renamed from canonical)
        if let Some(canonical) = &html_meta.canonical_url {
            println!("Canonical URL: {}", canonical);
        }

        // Access Open Graph fields as a map
        if let Some(og_image) = html_meta.open_graph.get("image") {
            println!("Open Graph Image: {}", og_image);
        }
        if let Some(og_title) = html_meta.open_graph.get("title") {
            println!("Open Graph Title: {}", og_title);
        }

        // Access Twitter Card fields as a map
        if let Some(twitter_card) = html_meta.twitter_card.get("card") {
            println!("Twitter Card Type: {}", twitter_card);
        }

        // Access new fields
        if let Some(lang) = &html_meta.language {
            println!("Language: {}", lang);
        }

        // Access headers
        if !html_meta.headers.is_empty() {
            for header in &html_meta.headers {
                println!("Header (level {}): {}", header.level, header.text);
            }
        }

        // Access links
        if !html_meta.links.is_empty() {
            for link in &html_meta.links {
                println!("Link: {} ({})", link.href, link.text);
            }
        }

        // Access images
        if !html_meta.images.is_empty() {
            for image in &html_meta.images {
                println!("Image: {}", image.src);
            }
        }

        // Access structured data
        if !html_meta.structured_data.is_empty() {
            println!("Structured data items: {}", html_meta.structured_data.len());
        }
    }
    Ok(())
}
```

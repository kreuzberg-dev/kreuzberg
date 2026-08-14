```csharp title="C#"
using Xberg;

var config = new ExtractionConfig
{
    PdfOptions = new PdfConfig { ExtractMetadata = true }
};

var result = (await XbergConverter.ExtractAsync(ExtractInput.FromUri("document.pdf"), config)).Results[0];

if (result.Metadata?.Format is FormatMetadata.Pdf pdfFormat)
{
    var pdfMeta = pdfFormat.Value;
    Console.WriteLine($"Pages: {pdfMeta.PageCount}");
    Console.WriteLine($"Author: {string.Join(", ", result.Metadata.Authors ?? new List<string>())}");
    Console.WriteLine($"Title: {result.Metadata.Title}");
}

var htmlResult = (await XbergConverter.ExtractAsync(ExtractInput.FromUri("page.html"), config)).Results[0];
if (htmlResult.Metadata?.Format is FormatMetadata.Html htmlFormat)
{
    var htmlMeta = htmlFormat.Value;
    Console.WriteLine($"Title: {htmlMeta.Title}");
    Console.WriteLine($"Description: {htmlMeta.Description}");

    // Access keywords as array
    if (htmlMeta.Keywords != null && htmlMeta.Keywords.Count > 0)
    {
        Console.WriteLine($"Keywords: {string.Join(", ", htmlMeta.Keywords)}");
    }

    // Access canonical URL (renamed from canonical)
    if (htmlMeta.CanonicalUrl != null)
    {
        Console.WriteLine($"Canonical URL: {htmlMeta.CanonicalUrl}");
    }

    // Access Open Graph fields from dictionary
    if (htmlMeta.OpenGraph != null && htmlMeta.OpenGraph.Count > 0)
    {
        if (htmlMeta.OpenGraph.ContainsKey("image"))
            Console.WriteLine($"Open Graph Image: {htmlMeta.OpenGraph["image"]}");
        if (htmlMeta.OpenGraph.ContainsKey("title"))
            Console.WriteLine($"Open Graph Title: {htmlMeta.OpenGraph["title"]}");
        if (htmlMeta.OpenGraph.ContainsKey("type"))
            Console.WriteLine($"Open Graph Type: {htmlMeta.OpenGraph["type"]}");
    }

    // Access Twitter Card fields from dictionary
    if (htmlMeta.TwitterCard != null && htmlMeta.TwitterCard.Count > 0)
    {
        if (htmlMeta.TwitterCard.ContainsKey("card"))
            Console.WriteLine($"Twitter Card Type: {htmlMeta.TwitterCard["card"]}");
        if (htmlMeta.TwitterCard.ContainsKey("creator"))
            Console.WriteLine($"Twitter Creator: {htmlMeta.TwitterCard["creator"]}");
    }

    // Access new fields
    if (htmlMeta.Language != null)
        Console.WriteLine($"Language: {htmlMeta.Language}");

    if (htmlMeta.TextDirection != null)
        Console.WriteLine($"Text Direction: {htmlMeta.TextDirection}");

    // Access headers
    if (htmlMeta.Headers != null && htmlMeta.Headers.Count > 0)
        Console.WriteLine($"Headers: {string.Join(", ", htmlMeta.Headers.Select(h => h.Text))}");

    // Access links
    if (htmlMeta.Links != null && htmlMeta.Links.Count > 0)
    {
        foreach (var link in htmlMeta.Links)
            Console.WriteLine($"Link: {link.Href} ({link.Text})");
    }

    // Access images
    if (htmlMeta.Images != null && htmlMeta.Images.Count > 0)
        Console.WriteLine($"Images: {string.Join(", ", htmlMeta.Images.Select(i => i.Src))}");

    // Access structured data
    if (htmlMeta.StructuredData != null && htmlMeta.StructuredData.Count > 0)
        Console.WriteLine($"Structured Data items: {htmlMeta.StructuredData.Count}");
}
```

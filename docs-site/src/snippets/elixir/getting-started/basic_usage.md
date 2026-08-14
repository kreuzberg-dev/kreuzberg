```elixir title="Elixir"
# Basic document extraction workflow
# Load file -> extract -> access results

{:ok, output} = Xberg.extract(input: %Xberg.ExtractInput{kind: :uri, uri: "document.pdf"}, config: nil)

result = List.first(output.results)
IO.puts("Extracted Content:")
IO.puts(result.content)

IO.puts("\nMetadata:")
IO.puts("Format: #{inspect(result.metadata.format)}")
IO.puts("Tables found: #{length(result.tables)}")
```

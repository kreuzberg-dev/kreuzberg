```elixir title="Elixir"
alias Xberg.Plugin

# Word Count Post-Processor Plugin
# This post-processor automatically counts words in extracted content
# and adds the word count to the metadata.

defmodule MyApp.Plugins.WordCountProcessor do
  @behaviour Xberg.Plugin.PostProcessor
  require Logger

  @impl true
  def name do
    "WordCountProcessor"
  end

  @impl true
  def processing_stage do
    :post
  end

  @impl true
  def version do
    "1.0.0"
  end

  @impl true
  def initialize do
    :ok
  end

  @impl true
  def shutdown do
    :ok
  end

  @impl true
  def process(result, _options) do
    content = result["content"] || ""
    word_count = content
    |> String.split(~r/\s+/, trim: true)
    |> length()

    # Update metadata with word count
    metadata = Map.get(result, "metadata", %{})
    updated_metadata = Map.put(metadata, "word_count", word_count)

    {:ok, Map.put(result, "metadata", updated_metadata)}
  end
end

# Register the word count post-processor
Plugin.register_post_processor(:word_count_processor, MyApp.Plugins.WordCountProcessor)

# Example usage
result = %{
"content" => "The quick brown fox jumps over the lazy dog. This is a sample document with multiple words.",
"metadata" => %{
"source" => "document.pdf",
"pages" => 1
}
}

case MyApp.Plugins.WordCountProcessor.process(result, %{}) do
  {:ok, processed_result} ->
  word_count = processed_result["metadata"]["word_count"]
  IO.puts("Word count added: #{word_count} words")
  IO.inspect(processed_result, label: "Processed Result")

  {:error, reason} ->
  IO.puts("Processing failed: #{reason}")
end

# List all registered post-processors
{:ok, processors} = Plugin.list_post_processors()
IO.inspect(processors, label: "Registered Post-Processors")
```

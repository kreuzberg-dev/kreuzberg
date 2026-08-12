unless System.get_env("CRAWLBERG_ALLOW_PRIVATE_NETWORK") do
  System.put_env("CRAWLBERG_ALLOW_PRIVATE_NETWORK", "true")
end

# Erlang os:putenv does not propagate to the native C runtime that an FFI
# library reads via getenv. Push each value through the binding's set_env NIF
# (libc setenv) so the native side observes the same environment.
try do
  Xberg.Native.set_env("CRAWLBERG_ALLOW_PRIVATE_NETWORK", "true")
rescue
  _ -> :ok
end

# Tests construct absolute paths via test_documents_path, so no chdir needed.
# Set ALEF_TEST_DOCUMENTS_DIR for any custom code that may reference it.
unless System.get_env("ALEF_TEST_DOCUMENTS_DIR") do
  test_documents_dir = Path.expand("../../../test_documents", __DIR__)
  System.put_env("ALEF_TEST_DOCUMENTS_DIR", test_documents_dir)
end

# Start a named Finch pool before ExUnit configured to use HTTP/1 only.
# Tests pass `finch: AlefE2EFinch` on every Req call; the pool's protocol
# selection (via `pools.default.protocols: [:http1]`) is the canonical place
# to pin the wire protocol since Req rejects per-call `:connect_options` when
# `:finch` is set.
{:ok, _} = Finch.start_link(name: AlefE2EFinch, pools: %{:default => [protocols: [:http1]]})

ExUnit.start()

# Spawn mock-server binary and set MOCK_SERVER_URL for all tests.
#
# Two execution modes:
# 1. External mode (`alef test-apps run` parent): MOCK_SERVER_URL is already set.
#    Parse any MOCK_SERVERS JSON and set each MOCK_SERVER_<FIXTURE_ID> env var
#    so tests reading `MOCK_SERVER_<UPPER>` find the dedicated per-fixture URL.
#    Do NOT spawn our own server.
# 2. Standalone mode (direct `mix test` / `task elixir:smoke`): Build the
#    mock-server binary if it is missing, then spawn it, capture its URL, and
#    let it run for the duration of the test suite.
mock_server_bin = Path.expand("../../rust/target/release/mock-server", __DIR__)
fixtures_dir = Path.expand("../../../fixtures", __DIR__)

unless System.get_env("MOCK_SERVER_URL") do
  unless File.exists?(mock_server_bin) do
    # Build the mock-server from the e2e/rust/ crate that alef generated.
    manifest = Path.expand("../../rust/Cargo.toml", __DIR__)
    unless File.exists?(manifest) do
      raise "mock-server Cargo.toml not found at #{manifest}"
    end
    {_output, 0} =
      System.cmd("cargo", ["build", "--release", "--manifest-path", manifest, "--bin", "mock-server"],
        stderr_to_stdout: true)
    unless File.exists?(mock_server_bin) do
      raise "mock-server binary still missing after build: #{mock_server_bin}"
    end
  end

  port = Port.open({:spawn_executable, mock_server_bin}, [
    :binary,
    # Use a large line buffer (default 1024 truncates `MOCK_SERVERS={...}` lines for
    # fixture sets with many host-root routes, splitting them into `:noeol` chunks
    # that the prefix-match clauses below would never see).
    {:line, 65_536},
    args: [fixtures_dir]
  ])
  # Read startup lines: MOCK_SERVER_URL= then MOCK_SERVERS= (always emitted, possibly `{}`).
  # The standalone mock-server prints noisy stderr lines BEFORE the stdout sentinels;
  # selective receive ignores anything that doesn't match the two prefix patterns.
  # Each iteration only halts after the MOCK_SERVERS= line is processed.
  {url, _} =
    Enum.reduce_while(1..16, {nil, port}, fn _, {url_acc, p} ->
      receive do
        {^p, {:data, {:eol, "MOCK_SERVER_URL=" <> u}}} ->
          {:cont, {u, p}}

        {^p, {:data, {:eol, "MOCK_SERVERS=" <> json_val}}} ->
          System.put_env("MOCK_SERVERS", json_val)
          case Jason.decode(json_val) do
            {:ok, servers} ->
              Enum.each(servers, fn {fid, furl} ->
                System.put_env("MOCK_SERVER_#{String.upcase(fid)}", furl)
              end)

            _ ->
              :ok
          end

          {:halt, {url_acc, p}}
      after
        30_000 ->
          raise "mock-server startup timeout"
      end
    end)

  if url != nil do
    System.put_env("MOCK_SERVER_URL", url)
  end
end

# If MOCK_SERVER_URL was preset by a parent (alef test-apps run), expand its
# MOCK_SERVERS JSON into per-fixture MOCK_SERVER_<FIXTURE_ID> env vars so
# tests reading `MOCK_SERVER_<UPPER>` find the dedicated per-fixture URL
# (without this, tests fall back to the shared-server namespaced URL where
# origin-relative asset paths 404).
if System.get_env("MOCK_SERVERS") do
  case Jason.decode(System.get_env("MOCK_SERVERS")) do
    {:ok, servers} ->
      Enum.each(servers, fn {fid, furl} ->
        System.put_env("MOCK_SERVER_#{String.upcase(fid)}", furl)
      end)

    _ ->
      :ok
  end
end

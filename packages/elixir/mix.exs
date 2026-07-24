defmodule Xberg.MixProject do
  use Mix.Project

  def project do
    [
    app: :xberg,
    version: "1.0.0-rc.37",
    elixir: "~> 1.14",
    elixirc_paths: ["lib", Path.expand("../../packages/elixir/native/xberg_nif/src", __DIR__)],
    rustler_crates: [
    xberg_nif: [
    mode: :release,
    targets: [
    "aarch64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-gnu"
    ]
    ]
    ],
    description: "High-performance document intelligence library",
    package: package(),
    deps: deps()
    ]
  end

  defp package do
    [
    licenses: ["MIT"],
    links: %{"GitHub" => "https://github.com/xberg-io/xberg"},
    files:
    ~w(lib .formatter.exs mix.exs README* checksum-*.exs native/xberg_nif/Cargo.toml native/xberg_nif/Cargo.lock native/xberg_nif/src)
    ]
  end

  defp deps do
    [
    {:jason, "~> 1.4"},
    {:rustler, "~> 0.38", runtime: false},
    {:rustler_precompiled, "~> 0.9"},
    {:credo, "~> 1.7", only: [:dev, :test], runtime: false},
    {:ex_doc, "~> 0.40", only: :dev, runtime: false}
    ]
  end
end

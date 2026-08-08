%{
  configs: [
    %{
      name: "default",
      strict: true,
      parse_timeout: 5000,
      files: %{
        included: [
          "lib/",
          "src/",
          "test/",
          "web/",
          "apps/*/lib/",
          "apps/*/src/",
          "apps/*/test/",
          "apps/*/web/"
        ],
        excluded: [
          ~r"/_build/",
          ~r"/deps/",
          ~r"/node_modules/"
        ]
      },
      checks: %{
        enabled: [
          {Credo.Check.Refactor.CyclomaticComplexity, max_complexity: 16}
        ]
      }
    }
  ]
}

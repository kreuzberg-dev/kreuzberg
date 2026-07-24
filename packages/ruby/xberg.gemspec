# frozen_string_literal: true

Gem::Specification.new do |spec|
  spec.name = "xberg"
  spec.version = "1.0.0.pre.rc.37"
  spec.authors       = ["Na'aman Hirschfeld <naaman@xberg.io>"]
  spec.summary       = "High-performance document intelligence library"
  spec.description   = "High-performance document intelligence library"
  spec.homepage      = "https://github.com/xberg-io/xberg"

  spec.license       = "MIT"

  spec.required_ruby_version = ">= 3.2.0"
  spec.metadata["keywords"] = %w[document extraction ocr pdf text].join(",")
  spec.metadata["rubygems_mfa_required"] = "true"

  candidate_files    = Dir.glob(%w[README* LICENSE* lib/**/* ext/**/* sig/**/* Steepfile]).select { |f| File.file?(f) }
  spec.files         = candidate_files.reject { |f| f.include?("/native/target/") || f.include?("/native/tmp/") }
  spec.require_paths = ["lib"]
  spec.extensions    = ["ext/xberg_rb/native/extconf.rb"]

  spec.add_dependency "rb_sys", ">= 0.9", "< 0.9.128"
  spec.add_dependency "sorbet-runtime", "~> 0.5"
end

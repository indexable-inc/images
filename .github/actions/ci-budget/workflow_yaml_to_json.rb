require "json"
require "yaml"

document = YAML.safe_load($stdin.read, aliases: true)
puts JSON.generate(document)

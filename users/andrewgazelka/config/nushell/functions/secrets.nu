# Secrets management with 24-hour caching.
#
# Two reference schemes are supported per template line:
#   - bw://<folder>/<item>/<field>  resolved via the rbw Vaultwarden CLI
#   - {{ op://<vault>/<item>/<field> }}  resolved via `op inject` (1Password)
#
# Vaultwarden (rbw) is the canonical store for ix-infra secrets. Personal
# items that have not been migrated into Vaultwarden still resolve through
# 1Password; convert a line to bw:// only once the item exists in Vaultwarden.

const SECRETS_CACHE = "~/.cache/nushell/secrets.nuon"
const CACHE_MAX_AGE = 24hr

# Resolve a single `$env.NAME = "..."` template line to { name, value } or null.
# Picks the resolver from the reference scheme on the line.
def resolve-secret-line [line: string] {
    let bw = $line | parse '$env.{name} = "bw://{ref}"'
    if ($bw | is-not-empty) {
        let name = $bw.0.name
        let segments = $bw.0.ref | split row "/"
        if ($segments | length) != 3 {
            return null
        }
        # The folder segment (segments.0) is kept for parity with the ix repo's
        # bw://ix-infra/... convention, where production Vaultwarden items live in
        # an `ix-infra` rbw folder. This personal Vaultwarden account stores items
        # at the root, and passing `--folder ix-infra` there matches a different,
        # stale value, so we resolve by unique item name + field without --folder.
        let item = $segments.1
        let field = $segments.2
        let value = try {
            ^rbw get --field $field $item | str trim --right --char "\n"
        } catch {
            null
        }
        if $value != null and ($value | str length) > 0 {
            return { name: $name, value: $value }
        }
        return null
    }

    # Fall back to 1Password `op inject` for `{{ op://... }}` lines.
    if (which op | is-empty) {
        return null
    }
    let temp_path = (mktemp -t nushell-secret.XXXXXX)
    $line | save -f $temp_path
    let rendered = try {
        op inject -i $temp_path
    } catch {
        null
    }
    rm -f $temp_path
    if $rendered == null {
        return null
    }
    let parts = $rendered | parse '$env.{name} = "{value}"'
    if ($parts | is-not-empty) {
        return { name: $parts.0.name, value: $parts.0.value }
    }
    null
}

# Generate secrets cache from template.
def generate-secrets-cache [] {
    let cache_path = $SECRETS_CACHE | path expand
    mkdir ($cache_path | path dirname)

    let template_path = $env.NIX_PRIVATE_CONFIG_DIR | path join "nushell" "secrets.template.nu"

    open $template_path
    | lines
    | where ($it | str starts-with "$env.")
    | each { |line| resolve-secret-line $line }
    | compact
    | reduce -f {} { |row, acc| $acc | insert $row.name $row.value }
    | to nuon
    | save -f $cache_path
}

# Load secrets into environment (call from env.nu)
def --env load-secrets [] {
    let cache_path = $SECRETS_CACHE | path expand

    let cache_fresh = if ($cache_path | path exists) {
        (ls $cache_path | get 0.modified) > ((date now) - $CACHE_MAX_AGE)
    } else { false }

    if not $cache_fresh {
        generate-secrets-cache
    }

    open $cache_path | load-env
}

# Force refresh secrets cache
def --env refresh-secrets [] {
    rm -f ($SECRETS_CACHE | path expand)
    load-secrets
    print "Secrets refreshed (Vaultwarden via rbw, 1Password via op inject)"
}

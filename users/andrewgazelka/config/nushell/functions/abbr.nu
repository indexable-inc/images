# Abbreviation expansion module
# Provides testable functions for shell abbreviation expansion

# Check if a token is at command position within a buffer
# Command position = first token after start or command separator (| ; & { ()
export def is_command_position [buffer: string, token: string]: nothing -> bool {
    # Split by command separators to get the last command segment
    let last_segment = $buffer
        | split row '|' | last
        | split row ';' | last
        | split row '&' | last
        | split row '{' | last
        | split row '(' | last

    # Trim leading whitespace from the segment
    let trimmed_segment = $last_segment | str trim -l

    # Get the tokens of this segment
    let tokens = $trimmed_segment | split row ' '

    # Command position means only one token in the segment
    ($tokens | length) == 1 and ($tokens | first | default '') == $token
}

def flatten-node [
    prefix: string
    expansion_prefix: string
    name: string
    spec: any
]: nothing -> list<record<key: string, value: string>> {
    if (($spec | describe) | str starts-with "record") {
        let base = ($spec | get -o base | default "")
        let expansion = if ($base | is-empty) {
            $expansion_prefix
        } else if ($expansion_prefix | is-empty) {
            $base
        } else {
            $expansion_prefix + " " + $base
        }
        let here = if ($expansion | is-empty) {
            []
        } else {
            [{ key: ($prefix + $name), value: $expansion }]
        }
        let children = ($spec | get -o children | default {})
        let descendants = (
            $children
            | transpose key value
            | each {|row| flatten-node ($prefix + $name) $expansion $row.key $row.value }
            | flatten
        )
        $here ++ $descendants
    } else {
        let expansion = if ($expansion_prefix | is-empty) {
            $spec
        } else {
            $expansion_prefix + " " + $spec
        }
        [{ key: ($prefix + $name), value: $expansion }]
    }
}

export def flatten-abbreviations [abbreviations: record]: nothing -> record {
    $abbreviations
    | transpose key value
    | each {|row| flatten-node "" "" $row.key $row.value }
    | flatten
    | reduce -f {} {|row, acc| $acc | upsert $row.key $row.value }
}

# Expand abbreviation in buffer
# Returns record with { value: string, expanded: bool }
export def expand [
    buffer: string
    abbreviations: record
    anywhere_abbreviations: record
    --skip-placeholders  # Skip abbreviations containing %
]: nothing -> record<value: string, expanded: bool> {
    # Fast path: empty buffer
    if ($buffer | is-empty) {
        return { value: $buffer, expanded: false }
    }

    # Split by command separators to get the last command segment
    let last_segment = $buffer
        | split row '|' | last
        | split row ';' | last
        | split row '&' | last
        | split row '{' | last
        | split row '(' | last

    # Trim leading whitespace from the segment
    let trimmed_segment = $last_segment | str trim -l

    # Get the tokens of this segment
    let tokens = $trimmed_segment | split row ' '
    let num_tokens = $tokens | length
    let last_token = $tokens | last | default ''

    # Check command abbreviations (only expand at command position - first token)
    if $num_tokens == 1 {
        let expansion = $abbreviations | get -o $last_token
        if ($expansion | is-not-empty) {
            if $skip_placeholders and ('%' in $expansion) {
                return { value: $buffer, expanded: false }
            }

            let prefix_len = ($buffer | str length) - ($last_token | str length)
            let prefix = if $prefix_len > 0 {
                $buffer | str substring 0..<$prefix_len
            } else {
                ''
            }
            return { value: ($prefix + $expansion), expanded: true }
        }
    }

    # Check anywhere abbreviations (can expand at any position)
    let arg_expansion = $anywhere_abbreviations | get -o $last_token
    if ($arg_expansion | is-not-empty) {
        if $skip_placeholders and ('%' in $arg_expansion) {
            return { value: $buffer, expanded: false }
        }

        let word_len = $last_token | str length
        let prefix = $buffer | str substring 0..<(($buffer | str length) - $word_len)
        { value: ($prefix + $arg_expansion), expanded: true }
    } else {
        { value: $buffer, expanded: false }
    }
}

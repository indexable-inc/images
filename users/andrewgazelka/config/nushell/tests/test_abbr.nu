# Tests for abbreviation expansion
use std/assert

# Import the module under test
use ../functions/abbr.nu

# Test abbreviations
const TEST_ABBRS = {
    a: "ast-grep --pattern"
    gs: "git status"
    placeholder: "echo %"
}

const TEST_ANYWHERE = {
    h: "--help"
}

const TEST_NESTED_ABBRS = {
    hm: {
        base: "home-manager"
        children: {
            g: "generations"
            s: {
                base: "switch"
                children: {
                    f: "--flake ~/.config/nix"
                }
            }
        }
    }
}

# ============================================
# Command position detection tests
# ============================================

#[test]
def test_is_command_position_at_start [] {
    assert (abbr is_command_position "a" "a")
}

#[test]
def test_is_command_position_after_pipe [] {
    assert (abbr is_command_position "ls | a" "a")
}

#[test]
def test_is_command_position_after_semicolon [] {
    assert (abbr is_command_position "ls; a" "a")
}

#[test]
def test_is_command_position_after_ampersand [] {
    assert (abbr is_command_position "ls & a" "a")
}

#[test]
def test_is_command_position_after_brace [] {
    assert (abbr is_command_position "{ a" "a")
}

#[test]
def test_is_command_position_after_paren [] {
    assert (abbr is_command_position "abc (a" "a")
}

#[test]
def test_is_command_position_with_leading_space [] {
    assert (abbr is_command_position "ls |  a" "a")
}

#[test]
def test_not_command_position_as_argument [] {
    assert not (abbr is_command_position "abc a" "a")
}

#[test]
def test_not_command_position_as_second_arg [] {
    assert not (abbr is_command_position "ls -la a" "a")
}

#[test]
def test_not_command_position_after_command [] {
    assert not (abbr is_command_position "git a" "a")
}

#[test]
def test_expand_tmux_kill_session [] {
    let result = abbr expand "tk" { tk: "tmux kill-session -t" } {}
    assert equal $result.value "tmux kill-session -t"
    assert $result.expanded
}

# ============================================
# Expansion tests - command position
# ============================================

#[test]
def test_expand_at_start [] {
    let result = abbr expand "a" $TEST_ABBRS {}
    assert equal $result.value "ast-grep --pattern"
    assert $result.expanded
}

#[test]
def test_expand_after_pipe [] {
    let result = abbr expand "ls | a" $TEST_ABBRS {}
    assert equal $result.value "ls | ast-grep --pattern"
    assert $result.expanded
}

#[test]
def test_expand_after_semicolon [] {
    let result = abbr expand "echo hi; gs" $TEST_ABBRS {}
    assert equal $result.value "echo hi; git status"
    assert $result.expanded
}

#[test]
def test_expand_after_paren [] {
    let result = abbr expand "abc (a" $TEST_ABBRS {}
    assert equal $result.value "abc (ast-grep --pattern"
    assert $result.expanded
}

#[test]
def test_expand_after_pipe_with_spaces [] {
    let result = abbr expand "ls |   a" $TEST_ABBRS {}
    assert equal $result.value "ls |   ast-grep --pattern"
    assert $result.expanded
}

# ============================================
# No expansion tests - argument position
# ============================================

#[test]
def test_no_expand_as_argument [] {
    let result = abbr expand "abc a" $TEST_ABBRS {}
    assert equal $result.value "abc a"
    assert not $result.expanded
}

#[test]
def test_no_expand_as_second_argument [] {
    let result = abbr expand "ls -la a" $TEST_ABBRS {}
    assert equal $result.value "ls -la a"
    assert not $result.expanded
}

#[test]
def test_no_expand_after_command [] {
    let result = abbr expand "git a" $TEST_ABBRS {}
    assert equal $result.value "git a"
    assert not $result.expanded
}

#[test]
def test_no_expand_in_string_context [] {
    let result = abbr expand "echo a" $TEST_ABBRS {}
    assert equal $result.value "echo a"
    assert not $result.expanded
}

# ============================================
# Anywhere abbreviation tests
# ============================================

#[test]
def test_anywhere_expands_at_start [] {
    let result = abbr expand "h" {} $TEST_ANYWHERE
    assert equal $result.value "--help"
    assert $result.expanded
}

#[test]
def test_anywhere_expands_as_argument [] {
    let result = abbr expand "git h" {} $TEST_ANYWHERE
    assert equal $result.value "git --help"
    assert $result.expanded
}

#[test]
def test_anywhere_expands_after_pipe [] {
    let result = abbr expand "ls | grep h" {} $TEST_ANYWHERE
    assert equal $result.value "ls | grep --help"
    assert $result.expanded
}

# ============================================
# Placeholder tests
# ============================================

#[test]
def test_placeholder_expands_normally [] {
    let result = abbr expand "placeholder" $TEST_ABBRS {}
    assert equal $result.value "echo %"
    assert $result.expanded
}

#[test]
def test_placeholder_skipped_with_flag [] {
    let result = abbr expand "placeholder" $TEST_ABBRS {} --skip-placeholders
    assert equal $result.value "placeholder"
    assert not $result.expanded
}

# ============================================
# Edge cases
# ============================================

#[test]
def test_empty_buffer [] {
    let result = abbr expand "" $TEST_ABBRS {}
    assert equal $result.value ""
    assert not $result.expanded
}

#[test]
def test_unknown_abbreviation [] {
    let result = abbr expand "xyz" $TEST_ABBRS {}
    assert equal $result.value "xyz"
    assert not $result.expanded
}

#[test]
def test_partial_match_no_expand [] {
    # "as" should not match "a"
    let result = abbr expand "as" $TEST_ABBRS {}
    assert equal $result.value "as"
    assert not $result.expanded
}

#[test]
def test_nested_parens [] {
    let result = abbr expand "foo (bar (a" $TEST_ABBRS {}
    assert equal $result.value "foo (bar (ast-grep --pattern"
    assert $result.expanded
}

#[test]
def test_command_abbr_preferred_over_anywhere [] {
    # When both exist, command position should use command abbr
    let result = abbr expand "a" $TEST_ABBRS $TEST_ANYWHERE
    assert equal $result.value "ast-grep --pattern"
    assert $result.expanded
}

#[test]
def test_flatten_nested_abbreviations [] {
    let flattened = abbr flatten-abbreviations $TEST_NESTED_ABBRS
    assert equal $flattened.hm "home-manager"
    assert equal $flattened.hmg "home-manager generations"
    assert equal $flattened.hms "home-manager switch"
    assert equal $flattened.hmsf "home-manager switch --flake ~/.config/nix"
}

#[test]
def test_expand_flattened_nested_abbreviation [] {
    let result = abbr expand "hmg" (abbr flatten-abbreviations $TEST_NESTED_ABBRS) {}
    assert equal $result.value "home-manager generations"
    assert $result.expanded
}

# ============================================
# Test runner
# ============================================

export def run-all [] {
    print "Running abbreviation tests..."

    # Command position tests
    test_is_command_position_at_start; print "✓ is_command_position_at_start"
    test_is_command_position_after_pipe; print "✓ is_command_position_after_pipe"
    test_is_command_position_after_semicolon; print "✓ is_command_position_after_semicolon"
    test_is_command_position_after_ampersand; print "✓ is_command_position_after_ampersand"
    test_is_command_position_after_brace; print "✓ is_command_position_after_brace"
    test_is_command_position_after_paren; print "✓ is_command_position_after_paren"
    test_is_command_position_with_leading_space; print "✓ is_command_position_with_leading_space"
    test_not_command_position_as_argument; print "✓ not_command_position_as_argument"
    test_not_command_position_as_second_arg; print "✓ not_command_position_as_second_arg"
    test_not_command_position_after_command; print "✓ not_command_position_after_command"

    # Expansion tests
    test_expand_at_start; print "✓ expand_at_start"
    test_expand_after_pipe; print "✓ expand_after_pipe"
    test_expand_after_semicolon; print "✓ expand_after_semicolon"
    test_expand_after_paren; print "✓ expand_after_paren"
    test_expand_after_pipe_with_spaces; print "✓ expand_after_pipe_with_spaces"

    # No expansion tests
    test_no_expand_as_argument; print "✓ no_expand_as_argument"
    test_no_expand_as_second_argument; print "✓ no_expand_as_second_argument"
    test_no_expand_after_command; print "✓ no_expand_after_command"
    test_no_expand_in_string_context; print "✓ no_expand_in_string_context"

    # Anywhere tests
    test_anywhere_expands_at_start; print "✓ anywhere_expands_at_start"
    test_anywhere_expands_as_argument; print "✓ anywhere_expands_as_argument"
    test_anywhere_expands_after_pipe; print "✓ anywhere_expands_after_pipe"

    # Placeholder tests
    test_placeholder_expands_normally; print "✓ placeholder_expands_normally"
    test_placeholder_skipped_with_flag; print "✓ placeholder_skipped_with_flag"

    # Edge cases
    test_empty_buffer; print "✓ empty_buffer"
    test_unknown_abbreviation; print "✓ unknown_abbreviation"
    test_partial_match_no_expand; print "✓ partial_match_no_expand"
    test_nested_parens; print "✓ nested_parens"
    test_command_abbr_preferred_over_anywhere; print "✓ command_abbr_preferred_over_anywhere"
    test_flatten_nested_abbreviations; print "✓ flatten_nested_abbreviations"
    test_expand_flattened_nested_abbreviation; print "✓ expand_flattened_nested_abbreviation"

    print "\n✅ All 31 tests passed!"
}

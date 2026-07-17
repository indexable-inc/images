export def "path list" [] {
    let input = $in
    let kind = ($input | describe)

    if $kind == "nothing" {
        []
    } else if ($kind =~ '^(list|table)') {
        $input
    } else {
        [ $input ]
    }
}

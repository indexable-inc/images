def "nu-complete ix vms" [] {
    let result = (do { ^ix ls --json } | complete)
    if $result.exit_code != 0 { return [] }
    $result.stdout
    | from json
    | each { |vm| { value: $vm.name, description: $"($vm.status) ($vm.image)" } }
}

export extern "ix rm" [
    ...names: string@"nu-complete ix vms"
    --force(-f)
]

export extern "ix ssh" [
    name?: string@"nu-complete ix vms"
    ...args: string
]

export extern "ix describe" [
    name?: string@"nu-complete ix vms"
]

export extern "ix shell" [
    name?: string@"nu-complete ix vms"
]

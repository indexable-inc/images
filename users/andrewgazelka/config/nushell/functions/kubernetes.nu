# Kubernetes namespace function - queries all clusters by default in parallel
export def kns_raw [filter?: string --current(-c)] {
    if $current {
        # Query current cluster only
        let current_context = kubectl config current-context | str trim
        let namespaces = kubectl get namespaces -o json
        | from json
        | get items
        | select metadata.name status.phase metadata.creationTimestamp
        | rename NAME STATUS AGE
        | insert CLUSTER { $current_context }

        if ($filter | is-empty) {
            $namespaces
        } else {
            $namespaces | where NAME =~ $filter
        }
    } else {
        # Query all clusters in parallel (default) - using kctx for composability
        let contexts = kctx | get NAME

        $contexts | par-each { |ctx|
            try {
                kubectl get namespaces $"--context=($ctx)" -o json
                | from json
                | get items
                | select metadata.name status.phase metadata.creationTimestamp
                | rename NAME STATUS AGE
                | insert CLUSTER { $ctx }
            } catch {
                []
            }
        } | flatten | if ($filter | is-empty) { $in } else { $in | where NAME =~ $filter }
    }
}

export def kns [filter?: string --current(-c)] {
    kns_raw $filter --current=$current
    | insert DAYS_AGO { |row|
        (date now) - ($row.AGE | into datetime)
        | into int  # converts duration to nanoseconds
        | $in / 86_400_000_000_000  # nanoseconds per day
        | math round
    }
}

# List all kubectl contexts/clusters
export def kctx [] {
    # Use kubectl config view to get proper structured data
    let config = kubectl config view -o json | from json
    let current_context = kubectl config current-context | str trim

    $config.contexts | each { |ctx|
        {
            CURRENT: ($ctx.name == $current_context)
            NAME: $ctx.name
            CLUSTER: $ctx.context.cluster
            AUTHINFO: $ctx.context.user
            NAMESPACE: (if ($ctx.context | columns | 'namespace' in $in) { $ctx.context.namespace } else { "" })
        }
    }
}

# Switch kubectl context
export def kuse [context: string] {
    kubectl config use-context $context
}

# Get node resource usage for current cluster
export def knodes [] {
    kubectl top nodes --no-headers
    | lines
    | each { |line|
        let parts = $line | split row -r '\s+'
        {
            NAME: ($parts | get 0)
            CPU_CORES: (
                $parts
                | get 1
                | str replace 'm' ''
                | into int
                | $in / 1000
                | math round -p 2
            )
            CPU_PERCENT: (
                $parts
                | get 2
                | str replace '%' ''
                | into int
            )
            MEMORY_MI: (
                $parts
                | get 3
                | str replace 'Mi' ''
                | into int
            )
            MEMORY_PERCENT: (
                $parts
                | get 4
                | str replace '%' ''
                | into int
            )
        }
    }
}

# Get total node resources across all clusters
export def kclusters [] {
    let contexts = kctx | get NAME

    $contexts | par-each { |ctx|
        try {
            let nodes = kubectl top nodes --no-headers $"--context=($ctx)"
            | lines
            | each { |line|
                let parts = $line | split row -r '\s+'
                {
                    CPU_CORES: (
                        $parts
                        | get 1
                        | str replace 'm' ''
                        | into int
                        | $in / 1000
                    )
                    MEMORY_MI: (
                        $parts
                        | get 3
                        | str replace 'Mi' ''
                        | into int
                    )
                }
            }

            let total_cpu = $nodes | get CPU_CORES | math sum | math round -p 2
            let total_memory_gb = $nodes | get MEMORY_MI | math sum | $in / 1024 | math round -p 2
            let node_count = $nodes | length

            {
                CLUSTER: $ctx
                NODES: $node_count
                TOTAL_CPU_CORES: $total_cpu
                TOTAL_MEMORY_GB: $total_memory_gb
                STATUS: "Active"
            }
        } catch {
            {
                CLUSTER: $ctx
                NODES: 0
                TOTAL_CPU_CORES: 0
                TOTAL_MEMORY_GB: 0
                STATUS: "Unreachable"
            }
        }
    } | sort-by CLUSTER
}

# Get all nodes from all clusters
export def knodes-all [] {
    let contexts = kctx | get NAME

    $contexts | par-each { |ctx|
        try {
            kubectl top nodes --no-headers $"--context=($ctx)"
            | lines
            | each { |line|
                let parts = $line | split row -r '\s+'
                {
                    CLUSTER: $ctx
                    NAME: ($parts | get 0)
                    CPU_CORES: (
                        $parts
                        | get 1
                        | str replace 'm' ''
                        | into int
                        | $in / 1000
                        | math round -p 2
                    )
                    CPU_PERCENT: (
                        $parts
                        | get 2
                        | str replace '%' ''
                        | into int
                    )
                    MEMORY_MI: (
                        $parts
                        | get 3
                        | str replace 'Mi' ''
                        | into int
                    )
                    MEMORY_GB: (
                        $parts
                        | get 3
                        | str replace 'Mi' ''
                        | into int
                        | $in / 1024
                        | math round -p 2
                    )
                    MEMORY_PERCENT: (
                        $parts
                        | get 4
                        | str replace '%' ''
                        | into int
                    )
                }
            }
        } catch {
            []
        }
    } | flatten | sort-by CLUSTER NAME
}

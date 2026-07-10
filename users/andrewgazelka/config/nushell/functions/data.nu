# Data manipulation and processing functions

# Extract JSON from text input
export def findjson []: any -> any {
    # Match JSON objects {} or arrays []
    # This regex captures complete JSON structures by matching balanced braces/brackets
    let regex = '(\{(?:[^{}]|(?:\{[^{}]*\}))*\}|\[(?:[^\[\]]|(?:\[[^\[\]]*\]))*\])'
    let result: string = $in | parse --regex $regex | get capture0.0
    $result | from json
}

# Format numbers with digit grouping (recursive)
export def format_numbers []: any -> any {
    let data = $in
    let desc = $data | describe

    if $desc == "int" or $desc == "float" {
        return ($data | into string --group-digits)
    }

    if $desc =~ "list" {
        return ($data | each { format_numbers })
    }

    if $desc =~ "record" {
        return ($data | items {|k, v| {$k: ($v | format_numbers)} } | into record)
    }

    if $desc =~ "table" {
        return ($data | each { format_numbers })
    }

    return $data
}

# Replace a PostgreSQL table with new JSON data, assuming the same schema.
#
# This function takes JSON data (as a nushell table/list) and replaces an entire
# PostgreSQL table with that data. It uses a transaction to ensure atomicity:
# the table is truncated and then repopulated with the new data.
#
# The function assumes the input data has the same schema as the target table.
# PostgreSQL's `json_populate_recordset()` function is used to convert the JSON
# back into table rows.
#
# Parameters:
# - connection_string: PostgreSQL connection string (can use environment variables)
# - table_name: Name of the table to replace
# - data: The new data to insert (nushell table/list)
#
# Returns: Result of the psql command execution
export def replace-table [
    connection_string: string  # PostgreSQL connection string
    table_name: string         # Name of the table to replace
]: [list -> any] {
    let data = $in

    # Convert data to JSON for PostgreSQL consumption
    let json_data = ($data | to json)

    # Build the SQL command to replace the table
    let sql_command = $"
BEGIN;
TRUNCATE ($table_name);
INSERT INTO ($table_name)
SELECT * FROM json_populate_recordset(null::($table_name), stdin::json);
COMMIT;
"

    # Execute the command, piping JSON data to psql
    $json_data | psql $connection_string -c $sql_command
}

# Get Chrome tabs from Superglide API
export def tabs [] {
    {} | http post --content-type=application/json :3001/tools/chrome_list_tabs/execute | get model_output
}

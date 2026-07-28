# Upload an HTML file as a private GitHub Gist and return a gistpreview link
export def main [path: path] {
    let filename = ($path | path basename)
    let content = (open --raw $path)

    let body = {
        public: false
        files: {
            ($filename): {
                content: $content
            }
        }
    }

    let result = ($body | to json | gh api gists --input - | from json)
    let gist_id = $result.id

    let url = $"https://gistpreview.github.io/?($gist_id)/($filename)"
    $url | pbcopy
    $url
}

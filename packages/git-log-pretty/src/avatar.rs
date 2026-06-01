//! Turn a commit author into avatar `PNG` bytes, resolving GitHub logins and
//! caching aggressively so a single log view makes at most one network request
//! per unique author.
//!
//! Resolution is cheapest-first and every step ties the email to a real account:
//! an explicit `git config githubLogin.map` override, then the login embedded in
//! a `noreply` email, then GitHub's record of who authored the commit. The
//! async fetches are driven by the caller's tokio runtime; everything else is
//! synchronous.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use git2::Repository;
use github_avatar::Client;

/// Name of the on-disk `email\tlogin` resolution cache within the cache dir.
const LOGIN_CACHE_FILE: &str = "logins.tsv";

/// Mask keeping image ids within the 24 bits the kitty Unicode-placeholder
/// foreground color can encode (`kitty::placeholder_row`).
const ID_MASK: u32 = 0x00FF_FFFF;

/// An author's avatar, ready to hand to the renderer.
pub struct Avatar {
    /// `PNG`-encoded image bytes.
    pub png: Vec<u8>,
    /// A stable per-author id so repeated commits reuse one transmitted image.
    pub id: u32,
}

/// Resolves and fetches author avatars, with in-memory and on-disk caches.
pub struct Resolver {
    client: Client,
    /// `owner/repo` parsed from the `origin` remote, if it is a GitHub remote.
    origin: Option<(String, String)>,
    /// Explicit `email -> login` overrides from `git config githubLogin.map`.
    identities: HashMap<String, String>,
    size_px: u32,
    /// Resolved `email -> login` (`None` = tried and failed), bounding requests.
    /// Successful resolutions are also persisted to disk and reloaded next run.
    login_cache: HashMap<String, Option<String>>,
    /// Downloaded `login -> png` (`None` = tried and failed).
    png_cache: HashMap<String, Option<Vec<u8>>>,
    /// Stable display ids assigned per login.
    login_ids: HashMap<String, u32>,
    next_id: u32,
    cache_dir: Option<PathBuf>,
}

impl Resolver {
    /// Build a resolver for `repo`, reading the `origin` remote, the identity-map
    /// overrides, a GitHub token, and the persisted login cache.
    #[must_use]
    pub fn new(repo: &Repository, size_px: u32) -> Self {
        let origin = repo
            .find_remote("origin")
            .ok()
            .and_then(|remote| remote.url().and_then(github_avatar::parse_remote));

        let cache_dir = avatar_cache_dir();
        let login_cache = cache_dir
            .as_ref()
            .map(|dir| load_login_cache(&dir.join(LOGIN_CACHE_FILE)))
            .unwrap_or_default();

        Self {
            client: Client::new(discover_token()),
            origin,
            identities: load_identities(repo),
            size_px,
            login_cache,
            png_cache: HashMap::new(),
            login_ids: HashMap::new(),
            // Seed the kitty image-id counter from a hash of the pid so concurrent
            // tools sharing the terminal are unlikely to collide. Kept within the
            // 24 bits the placeholder encoding can carry (and never 0, which the
            // protocol reserves); see [`Resolver::next_image_id`].
            next_id: ((std::process::id().wrapping_mul(2_654_435_761) >> 8) & ID_MASK).max(1),
            cache_dir,
        }
    }

    /// Resolve and fetch the avatar for the author of `sha` (commit email
    /// `email`). Returns `None` when the author cannot be mapped to a GitHub
    /// account or the avatar cannot be fetched.
    pub async fn avatar_for(&mut self, email: &str, sha: &str) -> Option<Avatar> {
        let login = self.login_for(email, sha).await?;
        let png = self.png_for(&login).await?;
        let id = self.id_for(&login);
        Some(Avatar { png, id })
    }

    async fn login_for(&mut self, email: &str, sha: &str) -> Option<String> {
        let email = email.trim();
        if email.is_empty() {
            return None;
        }
        if let Some(login) = self.identities.get(email) {
            return Some(login.clone());
        }
        if let Some(cached) = self.login_cache.get(email) {
            return cached.clone();
        }

        let resolved = match github_avatar::parse_noreply(email) {
            Some(user) => Some(user.login),
            None => self.resolve_via_commit(sha).await,
        };

        self.login_cache.insert(email.to_string(), resolved.clone());
        // Persist positive resolutions so later runs skip the network. Negatives
        // stay in memory only: an unpushed commit can resolve once it lands.
        if let Some(login) = &resolved {
            self.persist_login(email, login);
        }
        resolved
    }

    async fn resolve_via_commit(&self, sha: &str) -> Option<String> {
        let (owner, repo) = self.origin.as_ref()?;
        self.client
            .resolve_commit(owner, repo, sha)
            .await
            .ok()
            .flatten()
            .map(|user| user.login)
    }

    async fn png_for(&mut self, login: &str) -> Option<Vec<u8>> {
        if let Some(cached) = self.png_cache.get(login) {
            return cached.clone();
        }
        let bytes = self.load_or_fetch_png(login).await;
        self.png_cache.insert(login.to_string(), bytes.clone());
        bytes
    }

    /// Read the avatar from the on-disk cache, or download and cache it. Returns
    /// `None` only when the download fails.
    async fn load_or_fetch_png(&self, login: &str) -> Option<Vec<u8>> {
        // A plain `is_some` check rather than `if let` / `match` on the Option:
        // the miss path is async, which `option_if_let_else`'s `map_or_else`
        // rewrite cannot express.
        let cached = self.read_disk(login);
        if cached.is_some() {
            return cached;
        }
        let bytes = self.client.avatar_png(login, self.size_px).await.ok()?;
        self.write_disk(login, &bytes);
        Some(bytes)
    }

    fn id_for(&mut self, login: &str) -> u32 {
        if let Some(id) = self.login_ids.get(login) {
            return *id;
        }
        let id = self.next_image_id();
        self.login_ids.insert(login.to_string(), id);
        id
    }

    /// Reserve the next image id: 24-bit (the placeholder foreground can carry
    /// no more) and never 0 (reserved by the protocol). Wraps on overflow, which
    /// only matters after ~16M authors in one process.
    const fn next_image_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = match (self.next_id + 1) & ID_MASK {
            0 => 1,
            next => next,
        };
        id
    }

    fn disk_path(&self, login: &str) -> Option<PathBuf> {
        let sanitized: String = login
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        Some(
            self.cache_dir
                .as_ref()?
                .join(format!("{sanitized}-{size}.png", size = self.size_px)),
        )
    }

    fn read_disk(&self, login: &str) -> Option<Vec<u8>> {
        std::fs::read(self.disk_path(login)?).ok()
    }

    fn write_disk(&self, login: &str, bytes: &[u8]) {
        let Some(path) = self.disk_path(login) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, bytes);
    }

    /// Append an `email\tlogin` line to the on-disk resolution cache.
    /// Best-effort: failures (no cache dir, read-only fs) are ignored.
    fn persist_login(&self, email: &str, login: &str) {
        // Tabs and newlines would corrupt the line format; such values never
        // come from a valid email or login, so skip them defensively.
        if email.contains(['\t', '\n']) || login.contains(['\t', '\n']) {
            return;
        }
        let Some(path) = self.cache_dir.as_ref().map(|dir| dir.join(LOGIN_CACHE_FILE)) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            use std::io::Write;
            let _ = writeln!(file, "{email}\t{login}");
        }
    }
}

/// Find a GitHub token for the authenticated API lookups: env first, then the
/// `gh` CLI. Returns `None` if neither is available (the avatar download itself
/// needs no token).
fn discover_token() -> Option<String> {
    for key in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    let output = std::process::Command::new("gh").args(["auth", "token"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!token.is_empty()).then_some(token)
}

/// Read `email -> login` overrides from `git config githubLogin.map`.
///
/// The key is a multivar where each value is `email=login`. Lets a user pin
/// their own avatar when their commit email is not linked to a GitHub account.
fn load_identities(repo: &Repository) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(config) = repo.config() else {
        return map;
    };
    // git2 treats the glob as a regex over the (lowercased) entry name; anchor
    // it so it matches only `githublogin.map` and not other keys.
    if let Ok(entries) = config.entries(Some("^githublogin\\.map$")) {
        let _ = entries.for_each(|entry| {
            if let Some((email, login)) = entry.value().and_then(|value| value.split_once('=')) {
                let (email, login) = (email.trim(), login.trim());
                if !email.is_empty() && !login.is_empty() {
                    map.insert(email.to_string(), login.to_string());
                }
            }
        });
    }
    map
}

/// The directory holding cached avatars and resolutions, or `None` when neither
/// `XDG_CACHE_HOME` nor `HOME` is set.
fn avatar_cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(base.join("git-log-pretty").join("avatars"))
}

/// Load the persisted `email -> login` resolutions (all positive) into a map.
/// Later lines win, so a re-resolved login supersedes an older one. Missing or
/// unreadable files yield an empty map.
fn load_login_cache(path: &Path) -> HashMap<String, Option<String>> {
    let mut map = HashMap::new();
    let Ok(contents) = std::fs::read_to_string(path) else {
        return map;
    };
    for line in contents.lines() {
        if let Some((email, login)) = line.split_once('\t')
            && !email.is_empty()
            && !login.is_empty()
        {
            map.insert(email.to_string(), Some(login.to_string()));
        }
    }
    map
}

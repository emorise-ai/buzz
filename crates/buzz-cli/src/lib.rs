pub mod agent_management;
mod client;
mod commands;
mod error;
mod links;
mod validate;

use clap::{Parser, Subcommand};
use client::BuzzClient;
use error::CliError;
use nostr::Keys;
use uuid::Uuid;

/// Run the Buzz CLI from raw arguments (including `argv[0]`).
///
/// Returns a process exit code (0 = success).
///
/// # Example
///
/// ```ignore
/// let code = buzz_cli::run_from_args(std::env::args()).await;
/// std::process::exit(code);
/// ```
pub async fn run_from_args<I, S>(args: I) -> i32
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString> + Clone,
{
    // Install ring as the process-level rustls CryptoProvider. Required because the
    // release workflow builds all binaries in one cargo invocation, which unifies
    // features across the workspace and enables *both* ring (from buzz-acp/buzz-dev-mcp)
    // and aws-lc-rs (from reqwest's rustls feature via hyper-rustls). With both on,
    // rustls cannot auto-select a provider, and any code that reaches
    // ClientConfig::builder() — specifically the WSS path in publish_ephemeral_event
    // used by `agents draft-create`, `agents draft-update`, and `users set-presence`
    // — panics at rustls crypto/mod.rs. The `let _ =` swallow is intentional: when
    // buzz-dev-mcp delegates to run_from_args, it has already installed ring; the
    // double-install returns Err and is harmless.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => {
            if e.use_stderr() {
                error::print_error(&CliError::Usage(e.to_string()));
                return 1;
            } else {
                // --help and --version: print normally (intentional human output)
                let _ = e.print();
                return 0;
            }
        }
    };
    match run(cli).await {
        Ok(()) => 0,
        Err(e) => {
            error::print_error(&e);
            error::exit_code(&e)
        }
    }
}

#[derive(Parser)]
#[command(
    name = "buzz",
    about = "Buzz CLI — interact with a Buzz relay",
    long_about = "\
Buzz CLI — interact with a Buzz relay

Configuration (flags override env vars):
  BUZZ_RELAY_URL     Relay base URL        [default: http://localhost:3000]
  BUZZ_PRIVATE_KEY   Nostr private key (hex or nsec)  [required]
  BUZZ_AUTH_TAG      NIP-OA auth tag JSON  [optional]

The 'pack' subcommand runs locally and does not require a relay connection.

Exit codes: 0=ok  1=bad input  2=relay/network error  3=auth error  4=other  5=write conflict
Errors are JSON on stderr: {\"error\": \"<category>\", \"message\": \"<detail>\"}"
)]
struct Cli {
    /// Relay URL (http:// or https://). Overrides BUZZ_RELAY_URL env var.
    #[arg(long, env = "BUZZ_RELAY_URL", default_value = "http://localhost:3000")]
    relay: String,

    /// Nostr private key (hex or nsec). This is the CLI's identity.
    #[arg(long, env = "BUZZ_PRIVATE_KEY", hide_env_values = true)]
    private_key: Option<String>,

    /// NIP-OA auth tag JSON (owner attestation). Injected into every signed event.
    #[arg(long, env = "BUZZ_AUTH_TAG", hide_env_values = true)]
    auth_tag: Option<String>,

    /// Output format: 'json' (default, full fields) or 'compact' (reduced fields).
    #[arg(long, value_enum, default_value = "json")]
    format: OutputFormat,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Clone, clap::ValueEnum)]
pub enum ChannelType {
    #[value(name = "stream")]
    Stream,
    #[value(name = "forum")]
    Forum,
}

impl std::fmt::Display for ChannelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stream => write!(f, "stream"),
            Self::Forum => write!(f, "forum"),
        }
    }
}

#[derive(Clone, clap::ValueEnum)]
pub enum ChannelVisibility {
    #[value(name = "open")]
    Open,
    #[value(name = "private")]
    Private,
}

impl std::fmt::Display for ChannelVisibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::Private => write!(f, "private"),
        }
    }
}

#[derive(Clone, clap::ValueEnum)]
pub enum PresenceStatus {
    #[value(name = "online")]
    Online,
    #[value(name = "away")]
    Away,
    #[value(name = "offline")]
    Offline,
}

#[derive(Clone, clap::ValueEnum)]
pub enum EmojiScope {
    #[value(name = "own")]
    Own,
    #[value(name = "workspace")]
    Workspace,
}

impl std::fmt::Display for PresenceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Online => write!(f, "online"),
            Self::Away => write!(f, "away"),
            Self::Offline => write!(f, "offline"),
        }
    }
}

/// Output format for read commands.
#[derive(Clone, clap::ValueEnum, Default)]
pub enum OutputFormat {
    /// Full normalized JSON (default)
    #[default]
    #[value(name = "json")]
    Json,
    /// Reduced fields for agent scanning
    #[value(name = "compact")]
    Compact,
}

#[derive(Subcommand)]
enum Cmd {
    /// Draft owner-reviewed agent creation and updates
    #[command(subcommand)]
    Agents(AgentsCmd),
    /// Send, read, search, and manage messages
    #[command(subcommand)]
    Messages(MessagesCmd),
    /// Create, configure, and manage channels
    #[command(subcommand)]
    Channels(ChannelsCmd),
    /// Get and set channel canvas documents
    #[command(subcommand)]
    Canvas(CanvasCmd),
    /// Add, remove, and list emoji reactions
    #[command(subcommand)]
    Reactions(ReactionsCmd),
    /// Manage your custom emoji set (workspace palette is the union of all members' sets)
    #[command(subcommand)]
    Emoji(EmojiCmd),
    /// List, open, and manage direct messages
    #[command(subcommand)]
    Dms(DmsCmd),
    /// Look up users and manage profiles and presence
    #[command(subcommand)]
    Users(UsersCmd),
    /// Create, trigger, and manage workflows
    #[command(subcommand)]
    Workflows(WorkflowsCmd),
    /// Read the activity feed
    #[command(subcommand)]
    Feed(FeedCmd),
    /// Publish notes and manage the social graph (NIP-01/02)
    #[command(subcommand)]
    Social(SocialCmd),
    /// Publish and edit long-form NIP-23 notes — team knowledge base
    #[command(subcommand)]
    Notes(NotesCmd),
    /// Announce and discover git repositories (NIP-34)
    #[command(subcommand)]
    Repos(ReposCmd),
    /// Create and manage multi-repo projects (NIP-MP)
    #[command(subcommand)]
    Projects(ProjectsCmd),
    /// Send, get, list, and set status on git patches (NIP-34)
    #[command(subcommand)]
    Patches(PatchesCmd),
    /// Create, get, list, and set status on git issues (NIP-34)
    #[command(subcommand)]
    Issues(IssuesCmd),
    /// Open, update, list, and set status on git pull requests (NIP-34)
    #[command(subcommand)]
    Pr(PrCmd),
    /// Upload and download relay Blossom media
    #[command(subcommand)]
    Media(MediaCmd),
    /// Upload files to the relay's Blossom store
    #[command(subcommand)]
    Upload(UploadCmd),
    /// Create, extend, and destroy this agent's remote computer (sandbox)
    #[command(subcommand)]
    Sandbox(SandboxCmd),
    /// Agent engram management — persistent memory per NIP-AE
    #[command(subcommand)]
    Mem(MemCmd),
    /// Persona pack operations (local, no relay connection needed)
    #[command(subcommand)]
    Pack(PackCmd),
    /// Community moderation — reports queue, bans, timeouts, audit trail
    #[command(subcommand)]
    Moderation(ModerationCmd),
    /// Save a taught skill (Teach a Task) as a signed kind:48202 event
    #[command(subcommand)]
    Skills(SkillsCmd),
}

#[derive(Subcommand)]
pub enum SkillsCmd {
    /// Publish a taught skill. Body JSON comes from --body-file (a path, or
    /// "-" for stdin); the signing key's own pubkey is recorded as the
    /// `agent` tag automatically.
    Save {
        /// Skill id — becomes the `d` tag.
        #[arg(long)]
        id: String,
        /// Human-readable name — becomes the `name` tag.
        #[arg(long)]
        name: String,
        /// Owner pubkey (hex) this skill is taught for — becomes the `p` tag.
        #[arg(long)]
        owner: String,
        /// Recording media URL, if this skill was captured from a screen
        /// recording — becomes the `recording` tag.
        #[arg(long)]
        recording: Option<String>,
        /// Path to the skill body JSON, or "-" to read it from stdin.
        #[arg(long)]
        body_file: String,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum RespondToArg {
    #[value(name = "owner-only")]
    OwnerOnly,
    #[value(name = "anyone")]
    Anyone,
}

impl RespondToArg {
    fn to_wire(self) -> String {
        match self {
            Self::OwnerOnly => "owner-only",
            Self::Anyone => "anyone",
        }
        .to_string()
    }
}

#[derive(Subcommand)]
pub enum AgentsCmd {
    /// Open a prefilled create-agent form in the owner's Buzz Desktop
    DraftCreate {
        /// Current channel UUID; the new agent is added here after save
        #[arg(long)]
        channel: String,
        /// Proposed agent name
        #[arg(long)]
        display_name: String,
        /// Proposed instructions; use '-' to read from stdin
        #[arg(long)]
        system_prompt: String,
    },
    /// Open a prefilled edit-agent form in the owner's Buzz Desktop
    DraftUpdate {
        /// Current channel UUID
        #[arg(long)]
        channel: String,
        /// Current name of the personal agent to update
        #[arg(long)]
        agent_name: String,
        #[arg(long)]
        display_name: Option<String>,
        /// Replacement instructions; use '-' to read from stdin
        #[arg(long)]
        system_prompt: Option<String>,
        #[arg(long)]
        runtime: Option<String>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_enum)]
        respond_to: Option<RespondToArg>,
    },
    /// Submit a NIP-IA archive request for an identity (kind 9035)
    #[command(
        after_help = "Auth flow: when target != signer, the CLI fetches the target's kind:0 and \
attaches its owner-auth tag. On extraction failure it retries once (common cause: profile \
republish in progress). If the retry also fails, the command exits with an error — use \
--admin to bypass this guard when your key is a relay admin.\n\n\
Suggested --reason codes (unknown values are allowed): rotated, retired, \
bot-rebuilt, left-organization, spam\n\n\
Archiving a third-party identity is a human owner/admin action: an agent \
running under BUZZ_AUTH_TAG signs as itself, so it can only ever satisfy \
the self path (target == signer) — not the owner-of-agent path for another \
identity.\n\n\
Examples:\n  \
buzz agents archive <PUBKEY> --reason retired\n  \
buzz agents archive <PUBKEY> --reason bot-rebuilt --replaced-by <NEW_PUBKEY>"
    )]
    Archive {
        /// Target identity pubkey (hex)
        target_pubkey: String,
        /// Machine-readable reason code, max 64 UTF-8 bytes
        #[arg(long)]
        reason: Option<String>,
        /// Rotation pointer pubkey (hex); must differ from the target
        #[arg(long)]
        replaced_by: Option<String>,
        /// Optional human-readable note (not parsed for authorization)
        #[arg(long, default_value = "")]
        content: String,
        /// Allow sending without owner-auth attestation after extraction fails
        /// (relay-admin path). Use only when your key is a relay admin; ordinary
        /// owners do not need this flag. Without it, auth-extraction failure after
        /// one automatic retry is a hard error rather than a silent bare send.
        #[arg(long, default_value_t = false)]
        admin: bool,
    },
    /// Submit a NIP-IA unarchive request for an identity (kind 9036)
    #[command(
        after_help = "Auth flow: same as `archive` — retries kind:0 fetch once on \
extraction failure, then exits with an error if still unresolvable. Use --admin to bypass \
for relay-admin callers.\n\n\
Examples:\n  \
buzz agents unarchive <PUBKEY> --reason returned"
    )]
    Unarchive {
        /// Target identity pubkey (hex)
        target_pubkey: String,
        /// Machine-readable reason code, max 64 UTF-8 bytes
        #[arg(long)]
        reason: Option<String>,
        /// Optional human-readable note (not parsed for authorization)
        #[arg(long, default_value = "")]
        content: String,
        /// Allow sending without owner-auth attestation after extraction fails
        /// (relay-admin path). Use only when your key is a relay admin; ordinary
        /// owners do not need this flag. Without it, auth-extraction failure after
        /// one automatic retry is a hard error rather than a silent bare send.
        #[arg(long, default_value_t = false)]
        admin: bool,
    },
    /// Read the relay's current NIP-IA archive snapshot (kind 13535)
    #[command(
        after_help = "Verifies the snapshot's NIP-11 `self` authorship, event id, signature, \
and NIP-70 `-` protection tag before trusting it. Any trust failure is a \
nonzero-exit error, never a false-empty success — this command's whole \
purpose is verification.\n\n\
Examples:\n  \
buzz agents archived"
    )]
    Archived,
}

#[derive(Subcommand)]
pub enum MessagesCmd {
    /// Send a message to a channel
    #[command(
        after_help = "Examples:\n  buzz messages send --channel <UUID> --content \"hello\"\n  buzz messages send --channel <UUID> --content \"@alice check this\"\n  echo \"hello from stdin\" | buzz messages send --channel <UUID> --content -"
    )]
    Send {
        /// Channel UUID (from 'buzz channels list')
        #[arg(long)]
        channel: String,
        /// Message text — supports @mentions and markdown. Use '-' to read from stdin.
        #[arg(long)]
        content: String,
        /// Nostr event kind (default: channel default)
        #[arg(long)]
        kind: Option<u16>,
        /// Event ID to reply to (creates a thread)
        #[arg(long)]
        reply_to: Option<String>,
        /// Also publish to the Nostr network
        #[arg(long, default_value_t = false)]
        broadcast: bool,
        /// Attach file(s) — uploads and includes as imeta tags
        #[arg(long = "file")]
        files: Vec<String>,
        /// Pubkey to mention (hex or npub; repeatable). Supplying any explicit identity permits unresolved or ambiguous @Name text as presentation-only; uniquely resolved member names still notify.
        #[arg(long = "mention")]
        mentions: Vec<String>,
    },
    /// Send a code diff / patch to a channel
    SendDiff {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// Diff/patch content (use '-' to read from stdin)
        #[arg(long)]
        diff: String,
        /// Repository URL (e.g. https://github.com/org/repo)
        #[arg(long)]
        repo: String,
        /// Commit SHA
        #[arg(long)]
        commit: String,
        /// Single file path within the repo
        #[arg(long)]
        file: Option<String>,
        /// Parent commit SHA for three-way diff context
        #[arg(long)]
        parent_commit: Option<String>,
        /// Source branch name
        #[arg(long)]
        source_branch: Option<String>,
        /// Target branch name
        #[arg(long)]
        target_branch: Option<String>,
        /// Pull request number
        #[arg(long)]
        pr: Option<u32>,
        /// Language hint (auto-detected from file extension if omitted)
        #[arg(long)]
        lang: Option<String>,
        /// Human-readable description of the change
        #[arg(long)]
        description: Option<String>,
        /// Event ID to reply to (creates a thread)
        #[arg(long)]
        reply_to: Option<String>,
    },
    /// Edit a previously sent message
    Edit {
        /// Event ID of the message to edit (64-char hex)
        #[arg(long)]
        event: String,
        /// New message content
        #[arg(long)]
        content: String,
    },
    /// Delete a message by event ID
    Delete {
        /// Event ID to delete (64-char hex)
        #[arg(long)]
        event: String,
        /// Optional moderation audit action UUID for the public tombstone
        #[arg(long)]
        action_id: Option<Uuid>,
        /// Optional machine-readable public reason code for the tombstone
        #[arg(long)]
        reason_code: Option<String>,
        /// Optional human-readable public reason for the tombstone
        #[arg(long)]
        public_reason: Option<String>,
    },
    /// Retrieve messages from a channel
    #[command(
        after_help = "Examples:\n  buzz messages get --channel <UUID>\n  buzz messages get --channel <UUID> --limit 50 --kinds 1,1984"
    )]
    Get {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// Maximum number of results to return
        #[arg(long)]
        limit: Option<u32>,
        /// Unix timestamp — return messages before this time
        #[arg(long)]
        before: Option<i64>,
        /// Unix timestamp — return messages after this time
        #[arg(long)]
        since: Option<i64>,
        /// Comma-separated event kinds to filter (e.g. 1,1984)
        #[arg(long)]
        kinds: Option<String>,
    },
    /// Get the containing thread for a message or Buzz message link
    #[command(
        after_help = "Examples:\n  buzz messages thread --channel <UUID> --event <EVENT_ID>\n  buzz messages thread --link 'buzz://message?channel=<UUID>&id=<EVENT_ID>&thread=<ROOT_ID>'"
    )]
    Thread {
        /// Channel UUID; required unless --link is supplied
        #[arg(long, required_unless_present = "link", conflicts_with = "link")]
        channel: Option<String>,
        /// Message event ID (64-char hex); required unless --link is supplied
        #[arg(long, required_unless_present = "link", conflicts_with = "link")]
        event: Option<String>,
        /// Canonical buzz://message deep link; uses the configured relay and identity
        #[arg(long, conflicts_with_all = ["channel", "event"])]
        link: Option<String>,
        /// Maximum number of results to return
        #[arg(long)]
        limit: Option<u32>,
        /// Maximum reply nesting depth to include
        #[arg(long)]
        depth_limit: Option<u32>,
    },
    /// Full-text search across messages
    #[command(
        after_help = "Examples:\n  buzz messages search --query checkout\n  buzz messages search --author npub1... --since 1783497600\n  buzz messages search --author Aaron --query checkout --limit 20"
    )]
    Search {
        /// Search query string (optional when --author is given)
        #[arg(long)]
        query: Option<String>,
        /// Filter by author: 64-char hex pubkey, npub, or display name
        #[arg(long)]
        author: Option<String>,
        /// Unix timestamp — return messages after this time
        #[arg(long)]
        since: Option<i64>,
        /// Maximum number of results to return
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Upvote or downvote a forum post
    Vote {
        /// Event ID of the post to vote on (64-char hex)
        #[arg(long)]
        event: String,
        /// Vote direction: "up" or "down"
        #[arg(long)]
        direction: String,
    },
}

#[derive(Subcommand)]
pub enum ChannelsCmd {
    /// List channels visible to the current identity
    #[command(
        after_help = "Examples:\n  buzz channels list\n  buzz channels list --visibility open"
    )]
    List {
        /// Filter by visibility
        #[arg(long, value_enum)]
        visibility: Option<ChannelVisibility>,
        /// Only show channels where the current identity is a member
        #[arg(long, default_value_t = false)]
        member: bool,
        /// Maximum number of channels to return [default: 500]
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Get details for a single channel
    Get {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// Search channels by human-readable name
    #[command(
        after_help = "Examples:\n  buzz channels search --query composer\n  buzz channels search --query buzz-chat-composer --exact\n  buzz channels search --query design --include-archived"
    )]
    Search {
        /// Search query (case-insensitive substring of channel name)
        #[arg(long)]
        query: String,
        /// Require an exact case-insensitive match instead of substring
        #[arg(long, default_value_t = false)]
        exact: bool,
        /// Include archived channels in results
        #[arg(long, default_value_t = false)]
        include_archived: bool,
        /// Maximum number of channel-metadata events to fetch from the relay
        #[arg(long, default_value_t = 1000)]
        limit: u32,
    },
    /// Create a new channel
    #[command(
        after_help = "Examples:\n  buzz channels create --name general --type stream --visibility open\n  buzz channels create --name design --type forum --visibility open --description \"Design discussions\"\n  buzz channels create --name standup --type stream --visibility open --ttl 3600  # ephemeral, archived after 1h idle\n  buzz channels create --name project-x --template \"Buzz Team\"  # type/visibility/canvas/roster from the template; explicit flags override"
    )]
    Create {
        /// Channel name
        #[arg(long)]
        name: String,
        /// Channel type. Required unless --template supplies one.
        #[arg(long = "type", value_enum, required_unless_present = "template")]
        channel_type: Option<ChannelType>,
        /// Channel visibility. Required unless --template supplies one.
        #[arg(long, value_enum, required_unless_present = "template")]
        visibility: Option<ChannelVisibility>,
        /// Channel description
        #[arg(long)]
        description: Option<String>,
        /// Make the channel temporary/ephemeral: idle lifetime in seconds. If
        /// omitted, the channel is permanent. The relay archives it once this
        /// many seconds pass without a new message.
        #[arg(long, value_name = "SECONDS")]
        ttl: Option<i64>,
        /// Apply a desktop-local channel template by name (case-insensitive):
        /// supplies default type/visibility/description/canvas, and resolves
        /// its agent roster against the relay to add as members.
        #[arg(long)]
        template: Option<String>,
        /// Override the channel-templates.json path (default: the desktop
        /// app's prod app-data dir). Mainly for the dev store or testing.
        #[arg(long, value_name = "PATH")]
        templates_file: Option<String>,
    },
    /// Update channel name, description, visibility, or ephemeral TTL
    #[command(
        after_help = "Examples:\n  buzz channels update --channel <uuid> --name general\n  buzz channels update --channel <uuid> --visibility open\n  buzz channels update --channel <uuid> --visibility private"
    )]
    Update {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// New channel name
        #[arg(long)]
        name: Option<String>,
        /// New channel description
        #[arg(long)]
        description: Option<String>,
        /// New channel visibility
        #[arg(long, value_enum)]
        visibility: Option<ChannelVisibility>,
        /// Make the channel ephemeral (or change its lifetime): seconds until
        /// the relay archives it after the last message. Conflicts with --no-ttl.
        #[arg(long, value_name = "SECONDS", conflicts_with = "no_ttl")]
        ttl: Option<i64>,
        /// Clear an existing TTL, making the channel permanent.
        #[arg(long)]
        no_ttl: bool,
    },
    /// Set the channel topic
    Topic {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// New topic text
        #[arg(long)]
        topic: String,
    },
    /// Set the channel purpose
    Purpose {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// New purpose text
        #[arg(long)]
        purpose: String,
    },
    /// Join a channel
    Join {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// Leave a channel
    Leave {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// Archive a channel
    Archive {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// Unarchive a channel
    Unarchive {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// Delete a channel permanently
    Delete {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// List members of a channel
    Members {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// Add a member to a channel
    #[command(name = "add-member")]
    AddMember {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// Member pubkey (64-char hex)
        #[arg(long)]
        pubkey: String,
        /// Member role (owner, admin, member, guest, bot)
        #[arg(long)]
        role: Option<String>,
    },
    /// Remove a member from a channel
    #[command(name = "remove-member")]
    RemoveMember {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// Member pubkey (64-char hex)
        #[arg(long)]
        pubkey: String,
    },
    /// Set your channel addition policy
    #[command(name = "set-add-policy")]
    SetAddPolicy {
        /// Policy: anyone | owner_only | nobody
        #[arg(long)]
        policy: String,
    },
}

#[derive(Subcommand)]
pub enum CanvasCmd {
    /// Get the canvas document for a channel
    Get {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// Set (replace) the canvas document for a channel
    Set {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// Canvas content (markdown; use '-' to read from stdin)
        #[arg(long)]
        content: String,
    },
}

#[derive(Subcommand)]
pub enum ReactionsCmd {
    /// Add an emoji reaction to a message
    Add {
        /// Event ID (64-char hex)
        #[arg(long)]
        event: String,
        /// Emoji character (e.g. '👍') or custom emoji shortcode
        #[arg(long)]
        emoji: String,
        /// Image URL for a custom emoji reaction; when set, content becomes `:shortcode:`
        #[arg(long = "emoji-url")]
        emoji_url: Option<String>,
    },
    /// Remove an emoji reaction from a message
    Remove {
        /// Event ID (64-char hex)
        #[arg(long)]
        event: String,
        /// Emoji character to remove
        #[arg(long)]
        emoji: String,
    },
    /// List reactions on a message
    Get {
        /// Event ID (64-char hex)
        #[arg(long)]
        event: String,
    },
}

#[derive(Subcommand)]
pub enum EmojiCmd {
    /// List the workspace custom emoji palette (union of every member's set)
    List,
    /// Add or update a custom emoji in your own set
    Set {
        /// Emoji shortcode, without surrounding colons
        #[arg(long)]
        shortcode: String,
        /// Image URL for the emoji
        #[arg(long)]
        url: String,
    },
    /// Remove a custom emoji from your own set
    Rm {
        /// Emoji shortcode, without surrounding colons
        #[arg(long)]
        shortcode: String,
    },
    /// Export custom emojis to stdout or a file
    Export {
        /// Write JSON to this file path instead of stdout
        #[arg(long)]
        file: Option<String>,
        /// Export your own set (default) or the full workspace palette
        #[arg(long, value_enum, default_value = "own")]
        scope: EmojiScope,
    },
    /// Import custom emojis from stdin or a file into your own set
    Import {
        /// Read JSON from this file path instead of stdin
        #[arg(long)]
        file: Option<String>,
        /// Replace your entire set instead of merging
        #[arg(long, default_value_t = false)]
        replace: bool,
        /// Print what would be published without writing
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
pub enum DmsCmd {
    /// List direct message conversations
    List {
        /// Maximum number of results to return
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Open a new direct message with one or more users
    Open {
        /// User pubkey(s) to DM (64-char hex, 1-8)
        #[arg(long = "pubkey")]
        pubkeys: Vec<String>,
    },
    /// Add a member to an existing DM conversation
    AddMember {
        /// DM conversation UUID
        #[arg(long)]
        channel: String,
        /// User pubkey to add (64-char hex)
        #[arg(long)]
        pubkey: String,
    },
    /// Hide a DM conversation from your DM list
    Hide {
        /// DM conversation UUID
        #[arg(long)]
        channel: String,
    },
}

#[derive(Subcommand)]
pub enum UsersCmd {
    /// Look up user profiles by pubkey or name
    Get {
        /// User pubkey(s) to look up (64-char hex). Omit for your own profile
        #[arg(long = "pubkey")]
        pubkeys: Vec<String>,
        /// Search by display name (case-insensitive substring match)
        #[arg(long = "name")]
        name: Option<String>,
        /// Scope an exact-name agent lookup to its owner (`me`, hex, or npub)
        #[arg(long = "owner", requires = "name")]
        owner: Option<String>,
    },
    /// Update the current identity's profile
    #[command(name = "set-profile")]
    SetProfile {
        /// Display name
        #[arg(long)]
        name: Option<String>,
        /// Avatar URL
        #[arg(long)]
        avatar: Option<String>,
        /// Bio / about text
        #[arg(long)]
        about: Option<String>,
        /// NIP-05 identifier (e.g. user@example.com)
        #[arg(long)]
        nip05: Option<String>,
    },
    /// Get presence status for users
    Presence {
        /// Comma-separated pubkeys (64-char hex)
        #[arg(long)]
        pubkeys: String,
    },
    /// Set your presence status (online/away/offline)
    #[command(name = "set-presence")]
    SetPresence {
        /// Presence status
        #[arg(long, value_enum)]
        status: PresenceStatus,
    },
    /// Set your user status (NIP-38 kind:30315 — the "status" line on your profile)
    #[command(name = "set-status")]
    SetStatus {
        /// Status text (required unless --clear)
        #[arg(long, required_unless_present = "clear")]
        text: Option<String>,
        /// Optional emoji shown before the status text
        #[arg(long)]
        emoji: Option<String>,
        /// Remove your status entirely
        #[arg(long, conflicts_with_all = ["text", "emoji"])]
        clear: bool,
    },
}

#[derive(Subcommand)]
pub enum WorkflowsCmd {
    /// List workflows in a channel
    List {
        /// Channel UUID
        #[arg(long)]
        channel: String,
    },
    /// Get details for a single workflow
    Get {
        /// Workflow UUID
        #[arg(long)]
        workflow: String,
    },
    /// Create a workflow from a YAML definition
    Create {
        /// Channel UUID
        #[arg(long)]
        channel: String,
        /// Workflow YAML definition
        #[arg(long)]
        yaml: String,
    },
    /// Update a workflow's YAML definition
    Update {
        /// Channel UUID the workflow belongs to
        #[arg(long)]
        channel: String,
        /// Workflow UUID
        #[arg(long)]
        workflow: String,
        /// Updated workflow YAML definition
        #[arg(long)]
        yaml: String,
    },
    /// Delete a workflow
    Delete {
        /// Workflow UUID
        #[arg(long)]
        workflow: String,
    },
    /// Trigger a workflow run
    #[command(
        after_help = "Examples:\n  buzz workflows trigger --workflow <UUID>\n  buzz workflows trigger --workflow <UUID> --inputs '{\"key\":\"value\"}'"
    )]
    Trigger {
        /// Workflow UUID
        #[arg(long)]
        workflow: String,
        /// JSON object of input variables passed to the workflow as event content
        #[arg(long)]
        inputs: Option<String>,
    },
    /// List runs for a workflow
    Runs {
        /// Workflow UUID
        #[arg(long)]
        workflow: String,
        /// Maximum number of results to return
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Approve or deny a workflow step
    #[command(
        after_help = "Examples:\n  buzz workflows approve --token <UUID>\n  buzz workflows approve --token <UUID> --approved false --note \"needs revision\""
    )]
    Approve {
        /// The approval token UUID (from the approval request)
        #[arg(long)]
        token: String,
        /// Approve (true) or deny (false) the step
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        approved: bool,
        /// Optional note to include with the approval/denial
        #[arg(long)]
        note: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum FeedCmd {
    /// Get recent activity feed entries
    Get {
        /// Unix timestamp — return entries after this time
        #[arg(long)]
        since: Option<i64>,
        /// Maximum number of results to return
        #[arg(long)]
        limit: Option<u32>,
        /// Comma-separated feed types to include: mentions, needs_action, activity, agent_activity
        #[arg(long)]
        types: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum SocialCmd {
    /// Publish a text note (NIP-01 kind:1)
    #[command(name = "publish")]
    PublishNote {
        /// Text content of the note.
        #[arg(long)]
        content: String,
        /// 64-char hex event ID to reply to.
        #[arg(long)]
        reply_to: Option<String>,
    },
    /// Set your contact list (NIP-02 kind:3)
    #[command(name = "set-contacts")]
    SetContactList {
        /// JSON array of contacts: [{"pubkey":"hex","relay_url":"...","petname":"..."}]
        #[arg(long)]
        contacts: String,
    },
    /// Get a single event by ID
    #[command(name = "event")]
    GetEvent {
        /// 64-char hex event ID.
        #[arg(long)]
        event: String,
    },
    /// Get recent notes published by a user
    #[command(name = "notes")]
    GetUserNotes {
        /// 64-char hex pubkey of the author.
        #[arg(long)]
        pubkey: String,
        /// Maximum number of notes to return (default 50, max 100).
        #[arg(long)]
        limit: Option<u32>,
        /// Unix timestamp cursor — return notes created before this time.
        #[arg(long)]
        before: Option<i64>,
        /// Event ID cursor — return notes created before this event (composite pagination with --before).
        #[arg(long)]
        before_id: Option<String>,
    },
    /// Get a user's contact list
    #[command(name = "contacts")]
    GetContactList {
        /// 64-char hex pubkey.
        #[arg(long)]
        pubkey: String,
    },
    /// Publish a NIP-51/NIP-65 social list or set.
    #[command(name = "set-list")]
    SetList {
        /// Supported kind: 10000, 10001, 10002, 10003, 30000, or 30003.
        #[arg(long)]
        kind: u16,
        /// JSON array of Nostr tags, e.g. [["p","<hex>"],["d","friends"]].
        #[arg(long)]
        tags: String,
        /// Event content.
        #[arg(long, default_value = "")]
        content: String,
    },
    /// Get NIP-51/NIP-65 social lists or sets by author and kind.
    #[command(name = "list")]
    GetList {
        /// 64-char hex pubkey of the author.
        #[arg(long)]
        pubkey: String,
        /// Supported kind: 10000, 10001, 10002, 10003, 30000, or 30003.
        #[arg(long)]
        kind: u32,
        /// Optional d-tag for parameterized replaceable sets.
        #[arg(long)]
        d_tag: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum NotesCmd {
    /// Create or update a note. Idempotent upsert keyed by `(me, --name)`.
    ///
    /// `published_at` is preserved on edits (only set on first create).
    /// `--title` is required on first create; on subsequent edits the existing
    /// title is carried forward when `--title` is omitted, and `--title ""`
    /// explicitly clears it.
    #[command(
        after_help = "Examples:\n  echo '# Hello' | buzz notes set --name hello --title 'Hello' --content -\n  buzz notes set --name hello --tag onboarding --content - < draft.md"
    )]
    Set {
        /// Slug — becomes the `d` tag. `[a-z0-9._-]{1,80}`.
        #[arg(long)]
        name: String,
        /// Note title (NIP-23 `title` tag). Required on first create; omit to carry; `""` to clear.
        #[arg(long)]
        title: Option<String>,
        /// Short summary (NIP-23 `summary` tag). Omit to carry; `""` to clear.
        #[arg(long)]
        summary: Option<String>,
        /// Topic tag (NIP-23 `t` tag). May be repeated. Replaces (not merges) existing tags on edit; omit to carry forward.
        #[arg(long = "tag")]
        tags: Vec<String>,
        /// Clear all `t` tags on update. Mutually exclusive with `--tag`.
        /// Without this and without `--tag`, existing tags are carried forward.
        #[arg(long, default_value_t = false)]
        clear_tags: bool,
        /// Markdown body. Use `-` to read from stdin.
        #[arg(long)]
        content: String,
        /// Allow committing an empty body (refused by default to catch upstream pipeline failures).
        #[arg(long, default_value_t = false)]
        allow_empty: bool,
    },
    /// Read a note by `--naddr` (exact) or `--name <slug>` (cross-author lookup).
    Get {
        /// NIP-19 `naddr1…` or `30023:<pubkey>:<slug>` coordinate. Mutually exclusive with `--name`.
        #[arg(long)]
        naddr: Option<String>,
        /// Slug to look up across authors. Mutually exclusive with `--naddr`.
        #[arg(long)]
        name: Option<String>,
        /// Disambiguate `--name` to a specific author (hex pubkey, display name, or `me`).
        #[arg(long)]
        author: Option<String>,
        /// On an ambiguous `--name` (multiple authors), pick the most recently updated note
        /// instead of erroring. Mutually exclusive with `--author` and `--naddr`.
        #[arg(long, default_value_t = false)]
        latest: bool,
        /// Print only the markdown body, not the full event JSON.
        #[arg(long, default_value_t = false)]
        content_only: bool,
    },
    /// List notes. Defaults to your own.
    Ls {
        /// Hex pubkey, display name, `me`, or `all`.
        #[arg(long, default_value = "me")]
        author: Option<String>,
        /// Filter by NIP-23 `t` tag.
        #[arg(long)]
        tag: Option<String>,
        /// Max results (default 50, hard cap 200).
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Delete one of your own notes via NIP-09 (kind:5).
    ///
    /// Emits an a-tag-only deletion targeting the addressable coordinate
    /// `30023:<pubkey>:<slug>` (no `e` tag — an `e` tag would route around the
    /// relay's coordinate soft-delete and leave the note alive). Read-before-
    /// write gives a clean NotFound when there's nothing to delete.
    Rm {
        /// Slug of the note to delete. Only your own notes can be removed.
        #[arg(long)]
        name: String,
    },
}

#[derive(Subcommand)]
pub enum ReposCmd {
    /// Announce a git repository (NIP-34)
    Create {
        /// Repository identifier: [a-zA-Z0-9._-]{1,64}
        #[arg(long)]
        id: String,
        /// Human-readable display name
        #[arg(long)]
        name: Option<String>,
        /// Repository description
        #[arg(long)]
        description: Option<String>,
        /// Clone URL(s) — can be specified multiple times
        #[arg(long = "clone")]
        clone_urls: Vec<String>,
        /// Web browsing URL
        #[arg(long)]
        web: Option<String>,
        /// Preferred Nostr relay(s) for repo discovery — can be specified multiple times
        #[arg(long = "nostr-relay")]
        relays: Vec<String>,
        /// Channel UUID to bind the repo to. The `buzz-channel` tag is the
        /// git ACL: without it the relay 404s every clone/fetch/push until
        /// the author runs `buzz repos bind` (issue #3527).
        #[arg(long)]
        channel: Option<String>,
    },
    /// Get a repository announcement
    Get {
        /// Repository identifier (d-tag)
        #[arg(long)]
        id: String,
        /// Owner pubkey (64-char hex). Omit to match any owner.
        #[arg(long)]
        owner: Option<String>,
    },
    /// List repository announcements
    List {
        /// Owner pubkey (64-char hex). Omit for your repos.
        #[arg(long)]
        owner: Option<String>,
        /// Maximum number of results
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Bind (or rebind) one of your repositories to a channel.
    ///
    /// The `buzz-channel` tag on the announcement is the git ACL: the relay
    /// authorizes clone/fetch/push by membership in the bound channel. A
    /// repo announced without it (e.g. by a vanilla NIP-34 client) returns
    /// 404 for everyone until its author binds it here.
    Bind {
        /// Repository identifier (d-tag).
        #[arg(long)]
        id: String,
        /// Channel UUID to bind. Replaces any existing binding.
        #[arg(long)]
        channel: String,
    },
    /// Manage branch and tag protection rules on one of your repositories.
    #[command(subcommand)]
    Protect(ReposProtectCmd),
}

/// Commands for inspecting and changing repository protection rules.
#[derive(Subcommand)]
pub enum ReposProtectCmd {
    /// List the repository's protection rules.
    List {
        /// Repository identifier (d-tag).
        #[arg(long)]
        id: String,
    },
    /// Create or replace the rule for an exact ref pattern.
    Set {
        /// Repository identifier (d-tag).
        #[arg(long)]
        id: String,
        /// Full ref pattern, such as refs/heads/main or refs/heads/*.
        #[arg(long = "ref")]
        ref_pattern: String,
        /// Minimum role allowed to push.
        #[arg(long)]
        push: Option<RepoPushRole>,
        /// Reject non-fast-forward updates.
        #[arg(long, default_value_t = false)]
        no_force_push: bool,
        /// Reject deletion of matching refs.
        #[arg(long, default_value_t = false)]
        no_delete: bool,
        /// Require the NIP-34 patch workflow instead of direct pushes.
        #[arg(long, default_value_t = false)]
        require_patch: bool,
    },
    /// Remove every protection rule for an exact ref pattern.
    Remove {
        /// Repository identifier (d-tag).
        #[arg(long)]
        id: String,
        /// Full ref pattern to remove.
        #[arg(long = "ref")]
        ref_pattern: String,
    },
}

/// Minimum channel role accepted by a repository push rule.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum RepoPushRole {
    /// Repository owner only.
    Owner,
    /// Repository owner or channel admin.
    Admin,
    /// Any channel member.
    Member,
}

/// Visibility of a multi-repo project listing.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum ProjectVisibility {
    /// Project appears in public listings (default).
    Listed,
    /// Project is hidden from public listings.
    Unlisted,
}

impl ProjectVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            ProjectVisibility::Listed => "listed",
            ProjectVisibility::Unlisted => "unlisted",
        }
    }
}

#[derive(Subcommand)]
pub enum ProjectsCmd {
    /// Create a new multi-repo project (NIP-MP kind:30621)
    ///
    /// With no `--repo`, creates a default repository bound to `--channel`.
    /// Fails with Conflict if the project already exists.
    Create {
        /// Project identifier (slug), up to 1024 bytes
        slug: String,
        /// Member repository coordinate: bare Buzz repo id (e.g. `buzz`) or full
        /// `30617:<owner-hex>:<repo-d>` for cross-owner or colon-bearing repo ids.
        /// Omit to create a default repository named after the slug (requires `--channel`).
        #[arg(long = "repo")]
        repo: Vec<String>,
        /// Display name (≤256 bytes)
        #[arg(long)]
        name: Option<String>,
        /// Description (≤2048 bytes)
        #[arg(long)]
        description: Option<String>,
        /// Associated Buzz channel UUID
        #[arg(long)]
        channel: Option<String>,
        /// Visibility: `listed` (default) or `unlisted`
        #[arg(long)]
        visibility: Option<ProjectVisibility>,
    },
    /// Get a project by slug
    Get {
        /// Project slug
        slug: String,
        /// Owner pubkey (64-char hex). Defaults to the current identity.
        #[arg(long)]
        owner: Option<String>,
    },
    /// List projects
    List {
        /// Owner pubkey (64-char hex). Defaults to the current identity.
        #[arg(long)]
        owner: Option<String>,
        /// Maximum number of results
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Add one or more member repositories to a project
    #[command(name = "add-repo")]
    AddRepo {
        /// Project slug
        slug: String,
        /// Member repository coordinate (bare id or full `30617:<owner-hex>:<repo-d>`)
        #[arg(long = "repo", required = true)]
        repo: Vec<String>,
    },
    /// Draft a project-linked channel for owner review in Buzz Desktop
    #[command(name = "add-channel")]
    AddChannel {
        /// Project home channel UUID from the current ACP [Context]
        #[arg(long)]
        home_channel: String,
        /// New channel name
        #[arg(long)]
        name: String,
        /// Optional channel description
        #[arg(long)]
        description: Option<String>,
        /// Channel visibility
        #[arg(long, value_enum, default_value = "open")]
        visibility: ChannelVisibility,
        /// Optional temporary-channel lifetime in seconds
        #[arg(long)]
        ttl: Option<u64>,
        /// Optional Desktop channel-template name
        #[arg(long)]
        template: Option<String>,
    },
    /// Remove one or more member repositories from a project
    #[command(name = "remove-repo")]
    RemoveRepo {
        /// Project slug
        slug: String,
        /// Member repository coordinate to remove (bare id or full `30617:<owner-hex>:<repo-d>`)
        #[arg(long = "repo", required = true)]
        repo: Vec<String>,
    },
    /// Update project metadata (at least one setter or clearer required)
    #[command(group = clap::ArgGroup::new("mutation").required(true).multiple(true))]
    Update {
        /// Project slug
        slug: String,
        /// Set the display name
        #[arg(long, group = "mutation")]
        name: Option<String>,
        /// Remove the display name
        #[arg(long, group = "mutation", conflicts_with = "name")]
        clear_name: bool,
        /// Set the description
        #[arg(long, group = "mutation")]
        description: Option<String>,
        /// Remove the description
        #[arg(long, group = "mutation", conflicts_with = "description")]
        clear_description: bool,
        /// Set the associated Buzz channel UUID
        #[arg(long, group = "mutation")]
        channel: Option<String>,
        /// Remove the associated channel
        #[arg(long, group = "mutation", conflicts_with = "channel")]
        clear_channel: bool,
        /// Set visibility: `listed` or `unlisted`
        #[arg(long, group = "mutation")]
        visibility: Option<ProjectVisibility>,
        /// Remove the visibility tag (absence defaults to `listed`)
        #[arg(long, group = "mutation", conflicts_with = "visibility")]
        clear_visibility: bool,
    },
    /// Delete a project (head-based tombstone; verified after submit)
    Delete {
        /// Project slug
        slug: String,
    },
}

#[derive(Subcommand)]
pub enum PatchesCmd {
    /// Send a git patch (NIP-34 kind:1617)
    #[command(
        after_help = "Examples:\n  git format-patch -1 HEAD --stdout | buzz patches send --repo-owner <hex> --repo-id myrepo --patch-file - --root\n  buzz patches send --repo-owner <hex> --repo-id myrepo --patch-file 0001-fix.patch --reply-to <prev-patch-id>"
    )]
    Send {
        /// Repo owner pubkey (64-char hex)
        #[arg(long)]
        repo_owner: String,
        /// Repo identifier (d-tag)
        #[arg(long)]
        repo_id: String,
        /// Path to a `git format-patch` file, or '-' to read from stdin
        #[arg(long)]
        patch_file: String,
        /// Earliest-unique-commit of the repo
        #[arg(long)]
        euc: Option<String>,
        /// Additional recipient pubkey(s) — can be specified multiple times
        #[arg(long = "to")]
        to: Vec<String>,
        /// Previous patch event id (series) or original root (revision)
        #[arg(long)]
        reply_to: Option<String>,
        /// Mark as the first patch of a new series
        #[arg(long, default_value_t = false)]
        root: bool,
        /// Mark as the first patch of a new revision of an existing series
        #[arg(long, default_value_t = false)]
        root_revision: bool,
        /// Commit ID this patch produces when applied
        #[arg(long)]
        commit: Option<String>,
        /// Parent commit ID
        #[arg(long)]
        parent_commit: Option<String>,
        /// PGP signature of the commit
        #[arg(long)]
        commit_pgp_sig: Option<String>,
        /// Committer identity: 'name|email|timestamp|tz-offset-minutes'
        #[arg(long)]
        committer: Option<String>,
    },
    /// Get a patch by event id
    Get {
        /// Patch event id (64-char hex)
        #[arg(long)]
        event: String,
    },
    /// List patches for a repo
    List {
        /// Repo owner pubkey (64-char hex)
        #[arg(long)]
        repo_owner: String,
        /// Repo identifier (d-tag)
        #[arg(long)]
        repo_id: String,
        /// Filter by patch author pubkey
        #[arg(long)]
        author: Option<String>,
        /// Maximum number of results
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Set status on a patch (open/merged/closed/draft — NIP-34 kind:1630-1633)
    Status {
        /// Root patch event id (first patch of the series/revision)
        #[arg(long)]
        root: String,
        /// New status
        #[arg(long, value_parser = ["open", "merged", "closed", "draft"])]
        status: String,
        /// Markdown context for the status change ('-' to read from stdin)
        #[arg(long)]
        content: Option<String>,
        /// Repo owner pubkey — requires --repo-id
        #[arg(long, requires = "repo_id")]
        repo_owner: Option<String>,
        /// Repo identifier (d-tag) — requires --repo-owner
        #[arg(long, requires = "repo_owner")]
        repo_id: Option<String>,
        /// Earliest-unique-commit of the repo
        #[arg(long)]
        euc: Option<String>,
        /// Root id of the revision that was accepted (status=merged only)
        #[arg(long)]
        revision: Option<String>,
        /// Additional recipient pubkey(s) for the status event (besides the
        /// repo owner, which is tagged automatically when --repo-owner is
        /// given) — e.g. root/revision author. Can be specified multiple times.
        #[arg(long = "to")]
        to: Vec<String>,
        /// Applied patch event id — can be specified multiple times (status=merged only).
        /// Accepts `<id>`, `<id>:<relay-url>`, or `<id>:<relay-url>:<pubkey>`.
        #[arg(long = "q")]
        q: Vec<String>,
        /// Merge commit id (status=merged only)
        #[arg(long)]
        merge_commit: Option<String>,
        /// Commit id applied to the target branch — can be specified multiple times (status=merged only)
        #[arg(long = "applied-as-commit")]
        applied_as_commit: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum PrCmd {
    /// Open a git pull request (NIP-34 kind:1618)
    #[command(
        after_help = "Examples:\n  buzz pr open --repo-owner <hex> --repo-id myrepo --subject 'Fix bug' --body-file - --commit $(git rev-parse HEAD) --clone https://relay/git/owner/myrepo --branch-name fix-bug\n  buzz pr update --repo-owner <hex> --repo-id myrepo --pr <event> --pr-author <hex> --commit $(git rev-parse HEAD) --clone https://relay/git/owner/myrepo"
    )]
    Open {
        /// Repo owner pubkey (64-char hex)
        #[arg(long)]
        repo_owner: String,
        /// Repo identifier (d-tag)
        #[arg(long)]
        repo_id: String,
        /// Pull request subject/header
        #[arg(long, alias = "title")]
        subject: String,
        /// Pull request body markdown. Use '-' to read from stdin.
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        /// Path to pull request body markdown, or '-' to read from stdin.
        #[arg(long, conflicts_with = "body")]
        body_file: Option<String>,
        /// Tip commit of the PR branch
        #[arg(long)]
        commit: String,
        /// Clone URL where the tip commit can be fetched — can be specified multiple times
        #[arg(long = "clone", required = true)]
        clone: Vec<String>,
        /// Recommended branch name
        #[arg(long)]
        branch_name: Option<String>,
        /// Most recent common ancestor with the target branch
        #[arg(long)]
        merge_base: Option<String>,
        /// Earliest-unique-commit of the repo
        #[arg(long)]
        euc: Option<String>,
        /// Label — can be specified multiple times
        #[arg(long = "label")]
        label: Vec<String>,
        /// Additional recipient pubkey(s) — can be specified multiple times
        #[arg(long = "to")]
        to: Vec<String>,
        /// Channel where this pull request originated (NIP-29 h-tag)
        #[arg(long)]
        channel: Option<String>,
        /// Root patch event id this PR revises
        #[arg(long)]
        revision_of: Option<String>,
    },
    /// Update a git pull request tip (NIP-34 kind:1619)
    Update {
        /// Repo owner pubkey (64-char hex)
        #[arg(long)]
        repo_owner: String,
        /// Repo identifier (d-tag)
        #[arg(long)]
        repo_id: String,
        /// Pull request event id being updated
        #[arg(long)]
        pr: String,
        /// Pull request author's pubkey
        #[arg(long)]
        pr_author: String,
        /// Updated tip commit of the PR branch
        #[arg(long)]
        commit: String,
        /// Clone URL where the updated tip commit can be fetched — can be specified multiple times
        #[arg(long = "clone", required = true)]
        clone: Vec<String>,
        /// Markdown context for the update. Use '-' to read from stdin.
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        /// Path to markdown context for the update, or '-' to read from stdin.
        #[arg(long, conflicts_with = "body")]
        body_file: Option<String>,
        /// Most recent common ancestor with the target branch
        #[arg(long)]
        merge_base: Option<String>,
        /// Earliest-unique-commit of the repo
        #[arg(long)]
        euc: Option<String>,
        /// Additional recipient pubkey(s) — can be specified multiple times
        #[arg(long = "to")]
        to: Vec<String>,
    },
    /// Get a PR by event id
    Get {
        /// PR event id (64-char hex)
        #[arg(long)]
        event: String,
    },
    /// List PRs for a repo
    List {
        /// Repo owner pubkey (64-char hex)
        #[arg(long)]
        repo_owner: String,
        /// Repo identifier (d-tag)
        #[arg(long)]
        repo_id: String,
        /// Filter by PR author pubkey
        #[arg(long)]
        author: Option<String>,
        /// Filter by label
        #[arg(long)]
        label: Option<String>,
        /// Maximum number of results
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Set status on a PR (open/merged/closed/draft — NIP-34 kind:1630-1633)
    Status {
        /// Pull request event id
        #[arg(long)]
        pr: String,
        /// New status
        #[arg(long, value_parser = ["open", "merged", "closed", "draft"])]
        status: String,
        /// Markdown context for the status change. Use '-' to read from stdin.
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        /// Path to markdown context for the status change, or '-' to read from stdin.
        #[arg(long, conflicts_with = "body")]
        body_file: Option<String>,
        /// Repo owner pubkey — requires --repo-id
        #[arg(long, requires = "repo_id")]
        repo_owner: Option<String>,
        /// Repo identifier (d-tag) — requires --repo-owner
        #[arg(long, requires = "repo_owner")]
        repo_id: Option<String>,
        /// Earliest-unique-commit of the repo
        #[arg(long)]
        euc: Option<String>,
        /// Additional recipient pubkey(s) for the status event (besides the
        /// repo owner, which is tagged automatically when --repo-owner is
        /// given) — e.g. PR author/reviewers. Can be specified multiple times.
        #[arg(long = "to")]
        to: Vec<String>,
        /// Merge commit id (status=merged only)
        #[arg(long)]
        merge_commit: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum IssuesCmd {
    /// Create a git issue (NIP-34 kind:1621)
    Create {
        /// Repo owner pubkey (64-char hex). Optional when `--channel` (or
        /// `BUZZ_GIT_ORIGIN_CHANNEL_ID`) names a project home.
        #[arg(long)]
        repo_owner: Option<String>,
        /// Repo identifier (d-tag). Optional when `--channel` (or
        /// `BUZZ_GIT_ORIGIN_CHANNEL_ID`) names a project home.
        #[arg(long)]
        repo_id: Option<String>,
        /// Project home channel. Infers the repository, creating one bound to
        /// this project when none exists. Defaults to `BUZZ_GIT_ORIGIN_CHANNEL_ID`.
        #[arg(long)]
        channel: Option<String>,
        /// Issue title
        #[arg(long, alias = "subject")]
        title: String,
        /// Issue body, markdown. Use '-' to read from stdin.
        #[arg(long)]
        content: String,
        /// Label — can be specified multiple times
        #[arg(long = "label")]
        label: Vec<String>,
        /// Additional recipient pubkey(s) — can be specified multiple times
        #[arg(long = "to")]
        to: Vec<String>,
    },
    /// Get an issue by event id
    Get {
        /// Issue event id (64-char hex)
        #[arg(long)]
        event: String,
    },
    /// List issues for a repo
    List {
        /// Repo owner pubkey (64-char hex)
        #[arg(long)]
        repo_owner: String,
        /// Repo identifier (d-tag)
        #[arg(long)]
        repo_id: String,
        /// Filter by issue author pubkey
        #[arg(long)]
        author: Option<String>,
        /// Filter by label
        #[arg(long)]
        label: Option<String>,
        /// Maximum number of results
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Set status on an issue (open/resolved/closed/draft — NIP-34 kind:1630-1633)
    Status {
        /// Issue event id
        #[arg(long)]
        issue: String,
        /// New status
        #[arg(long, value_parser = ["open", "resolved", "closed", "draft"])]
        status: String,
        /// Markdown context for the status change ('-' to read from stdin)
        #[arg(long)]
        content: Option<String>,
        /// Repo owner pubkey — requires --repo-id
        #[arg(long, requires = "repo_id")]
        repo_owner: Option<String>,
        /// Repo identifier (d-tag) — requires --repo-owner
        #[arg(long, requires = "repo_owner")]
        repo_id: Option<String>,
        /// Earliest-unique-commit of the repo
        #[arg(long)]
        euc: Option<String>,
        /// Additional recipient pubkey(s) for the status event (besides the
        /// repo owner, which is tagged automatically when --repo-owner is
        /// given) — e.g. the issue author. Can be specified multiple times.
        #[arg(long = "to")]
        to: Vec<String>,
    },
    /// Assign an issue to one or more people or agents. Only assignments
    /// signed by the issue author or repo owner are trusted by clients;
    /// anyone may assign themselves (sole assignee = your own pubkey).
    Assign {
        /// Issue event id (64-char hex)
        #[arg(long)]
        issue: String,
        /// Repo owner pubkey (64-char hex)
        #[arg(long)]
        repo_owner: String,
        /// Repo identifier (d-tag)
        #[arg(long)]
        repo_id: String,
        /// Assignee pubkey (64-char hex) — can be specified multiple times
        #[arg(long = "assignee", required = true)]
        assignee: Vec<String>,
        /// Human-readable assignee name(s) for the note body, e.g. "Thomas".
        /// Defaults to the truncated assignee pubkeys.
        #[arg(long)]
        label: Option<String>,
    },
    /// Remove one or more assignees from an issue. Issue authors and repo
    /// owners may remove anyone; other users may remove only themselves.
    Unassign {
        /// Issue event id (64-char hex)
        #[arg(long)]
        issue: String,
        /// Repo owner pubkey (64-char hex)
        #[arg(long)]
        repo_owner: String,
        /// Repo identifier (d-tag)
        #[arg(long)]
        repo_id: String,
        /// Assignee pubkey to remove — can be specified multiple times
        #[arg(long = "assignee", required = true)]
        assignee: Vec<String>,
        /// Human-readable assignee name(s) for the note body.
        /// Defaults to the truncated assignee pubkeys.
        #[arg(long)]
        label: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum UploadCmd {
    /// Upload a file to the relay's Blossom store
    File {
        /// Path to the file to upload
        #[arg(long)]
        file: String,
    },
}

/// Where the sandbox broker lives. Shared by every `buzz sandbox` variant so
/// the agent's injected environment answers it without a flag.
#[derive(clap::Args, Clone)]
pub struct SandboxBrokerArg {
    /// Sandbox broker base URL. Overrides BUZZ_SANDBOX_BROKER_URL.
    #[arg(long = "broker", env = "BUZZ_SANDBOX_BROKER_URL")]
    pub broker: String,
}

/// Which sandbox to act on. Shared by every computer-use variant — the
/// caller can name the id explicitly, or lean on `BUZZ_SANDBOX_ID`, which
/// the ACP harness injects into a managed agent's own sandbox process.
#[derive(clap::Args, Clone)]
pub struct SandboxIdArg {
    /// Sandbox id or name. Falls back to BUZZ_SANDBOX_ID.
    #[arg(env = "BUZZ_SANDBOX_ID")]
    pub id: Option<String>,
}

impl SandboxIdArg {
    /// Resolve the sandbox id: explicit positional, then `BUZZ_SANDBOX_ID`
    /// (both handled by clap's `env` fallback — this also covers the case
    /// where clap accepted an empty value), then self-detection from inside
    /// the container itself.
    ///
    /// `BUZZ_SANDBOX_ID` can't be injected at container creation — the
    /// broker doesn't know the container's id until after `docker create`
    /// returns, and env vars are fixed at creation. So a sandbox's own agent
    /// process, run without the env var wired in, falls back to reading its
    /// own hostname: Docker sets a container's hostname to its short id by
    /// default (verified: the broker sets no custom hostname), and the
    /// Docker API accepts unique id prefixes, so the hostname works
    /// unmodified as an id. Guarded so a non-hex hostname (e.g. a host
    /// process with `/.dockerenv` coincidentally present) never gets used.
    pub fn require(&self) -> Result<String, CliError> {
        resolve_sandbox_id(
            self.id.as_deref(),
            std::path::Path::new("/.dockerenv").exists(),
            read_hostname,
        )
    }
}

/// Pure resolution: explicit id, then self-detection gated on `in_container`,
/// using `hostname` to fetch the kernel hostname lazily (only read when
/// actually inside a container — avoids a needless syscall/file-read on the
/// host, and lets tests inject a fake hostname without touching the fs).
///
/// `in_container` stands in for `/.dockerenv`'s existence and `hostname` for
/// `/etc/hostname`'s contents so the precedence and hex-shape guard are
/// testable without a real container.
fn resolve_sandbox_id(
    explicit: Option<&str>,
    in_container: bool,
    hostname: impl FnOnce() -> Option<String>,
) -> Result<String, CliError> {
    if let Some(id) = explicit {
        if !id.is_empty() {
            return Ok(id.to_string());
        }
    }
    if in_container {
        if let Some(id) = hostname().and_then(|h| container_id_from_hostname(&h)) {
            return Ok(id);
        }
    }
    Err(CliError::Usage(
        "sandbox id required: pass it as an argument or set BUZZ_SANDBOX_ID \
         (inside a Buzz computer, the id is auto-detected from the container \
         hostname — this only fails outside a sandbox)"
            .to_string(),
    ))
}

/// Read the kernel hostname the same way `gethostname(2)` would, via the
/// `/proc`-free `/etc/hostname` file so no extra crate/FFI is needed.
fn read_hostname() -> Option<String> {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Validate (and normalize) a hostname as a Docker short container id:
/// `^[0-9a-f]{12,64}$` case-insensitively (normalized to lowercase). Docker
/// container ids are hex, 12-64 characters, and the daemon accepts any
/// unique prefix, so this is safe to hand straight to the broker as-is.
fn container_id_from_hostname(hostname: &str) -> Option<String> {
    let candidate = hostname.trim().to_ascii_lowercase();
    if (12..=64).contains(&candidate.len()) && candidate.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(candidate)
    } else {
        None
    }
}

#[derive(Subcommand)]
pub enum SandboxCmd {
    /// Create a sandbox — a remote desktop + browser owned by this key
    Create {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        /// Image to run (must be on the broker's allowlist)
        #[arg(long, env = "BUZZ_SANDBOX_IMAGE", default_value = "buzz-sprig-desktop")]
        image: String,
        /// Lifetime in seconds before the sandbox is reclaimed
        #[arg(long, default_value_t = 1800)]
        ttl: u64,
        /// CPU budget (broker clamps to its ceiling)
        #[arg(long)]
        cpus: Option<f64>,
        /// Memory budget in MB (broker clamps to its ceiling)
        #[arg(long)]
        memory_mb: Option<u64>,
    },
    /// List sandboxes the broker manages
    List {
        #[command(flatten)]
        broker: SandboxBrokerArg,
    },
    /// One sandbox's state, including its live expiry
    Status {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        /// Sandbox id or name
        id: String,
    },
    /// Buy a sandbox more time: lifetime becomes now + --ttl (owner only,
    /// capped at 8h total from creation)
    Extend {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        /// Sandbox id or name
        id: String,
        /// Seconds of lifetime wanted from now
        #[arg(long)]
        ttl: u64,
    },
    /// Destroy a sandbox now
    Destroy {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        /// Sandbox id or name
        id: String,
    },
    /// Run a command on the sandbox and capture its output
    Exec {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        #[command(flatten)]
        sandbox: SandboxIdArg,
        /// Seconds before the command is killed
        #[arg(long, default_value_t = 30)]
        timeout: u64,
        /// Working directory for the command
        #[arg(long)]
        workdir: Option<String>,
        /// Run this string via `bash -lc` instead of an argv (mutually
        /// exclusive with trailing arguments)
        #[arg(short = 'c', long, conflicts_with = "argv")]
        command: Option<String>,
        /// Command and arguments to run verbatim, e.g. `-- ls -la /workspace`
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        argv: Vec<String>,
    },
    /// Capture a PNG screenshot of the sandbox's screen
    Screenshot {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        #[command(flatten)]
        sandbox: SandboxIdArg,
        /// Output path. Default: ./sandbox-screenshot-<id8>-<unixts>.png
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Click at a screen coordinate
    Click {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        x: i64,
        y: i64,
        #[command(flatten)]
        sandbox: SandboxIdArg,
        /// Mouse button to click
        #[arg(long, default_value = "left")]
        button: String,
    },
    /// Double-click at a screen coordinate
    DoubleClick {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        x: i64,
        y: i64,
        #[command(flatten)]
        sandbox: SandboxIdArg,
    },
    /// Move the mouse to a screen coordinate
    Move {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        x: i64,
        y: i64,
        #[command(flatten)]
        sandbox: SandboxIdArg,
    },
    /// Type text at the current focus
    Type {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        text: String,
        #[command(flatten)]
        sandbox: SandboxIdArg,
    },
    /// Send a key combo, e.g. `ctrl+t`
    Key {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        combo: String,
        #[command(flatten)]
        sandbox: SandboxIdArg,
    },
    /// Scroll at a screen coordinate
    Scroll {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        x: i64,
        y: i64,
        #[command(flatten)]
        sandbox: SandboxIdArg,
        /// Scroll direction
        #[arg(long)]
        direction: String,
        /// Number of scroll steps
        #[arg(long, default_value_t = 1)]
        amount: u32,
    },
    /// Launch an app on the sandbox desktop
    Open {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        #[command(flatten)]
        sandbox: SandboxIdArg,
        /// App to launch
        #[arg(long)]
        app: String,
        /// URL to open (browser only)
        #[arg(long)]
        url: Option<String>,
    },
    /// Start recording the sandbox's screen to video
    RecordStart {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        #[command(flatten)]
        sandbox: SandboxIdArg,
    },
    /// Stop an in-progress screen recording and save it as an mp4
    RecordStop {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        #[command(flatten)]
        sandbox: SandboxIdArg,
        /// Output path. Default: ./sandbox-recording-<id8>-<unixts>.mp4
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Transfer files between this agent's workspace and its computer
    Files {
        #[command(subcommand)]
        command: SandboxFilesCmd,
    },
}

#[derive(Subcommand)]
pub enum SandboxFilesCmd {
    /// List a directory inside the computer
    List {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        #[command(flatten)]
        sandbox: SandboxIdArg,
        /// Absolute directory under /workspace or /home/agent
        #[arg(long, default_value = "/workspace")]
        path: String,
    },
    /// Copy a file from the computer into the agent's local workspace
    Get {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        /// Absolute source path under /workspace or /home/agent
        remote_path: String,
        #[command(flatten)]
        sandbox: SandboxIdArg,
        /// Local destination path (defaults to the remote file name)
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Copy a local workspace file into the computer
    Put {
        #[command(flatten)]
        broker: SandboxBrokerArg,
        /// Local source path
        local_path: String,
        /// Absolute destination path under /workspace or /home/agent
        remote_path: String,
        #[command(flatten)]
        sandbox: SandboxIdArg,
    },
}

#[derive(Subcommand)]
pub enum MediaCmd {
    /// Download relay media with Blossom get auth
    Get {
        /// Relay media URL or sha256[.ext] path segment
        input: String,
        /// Output path. Omit or use '-' to write raw bytes to stdout.
        #[arg(short, long)]
        output: Option<String>,
    },
}

/// Subcommands for `buzz mem`.
#[derive(Subcommand)]
pub enum MemCmd {
    /// List non-tombstoned memory entries
    Ls {
        /// Owner pubkey (hex). Overrides BUZZ_AUTH_TAG.
        #[arg(long)]
        owner: Option<String>,
        /// Agent pubkey (hex) to read as this key's owner.
        #[arg(long)]
        agent: Option<String>,
        /// Emit JSON instead of tab-delimited lines.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Print the value of a slug to stdout (no trailing newline)
    Get {
        slug: String,
        #[arg(long)]
        owner: Option<String>,
        /// Agent pubkey (hex) to read as this key's owner.
        #[arg(long)]
        agent: Option<String>,
    },
    /// Print sha256(value) in hex (use as `--base-hash` for `mem patch`).
    Hash {
        slug: String,
        #[arg(long)]
        owner: Option<String>,
        /// Agent pubkey (hex) to read as this key's owner.
        #[arg(long)]
        agent: Option<String>,
    },
    /// Set a slug's value. Pass `-` to read the value from stdin.
    Set {
        slug: String,
        value: String,
        #[arg(long)]
        owner: Option<String>,
        /// Allow committing an empty value. Without this, a zero-byte stdin
        /// read is rejected to prevent silent data loss from upstream
        /// pipeline failures.
        #[arg(long, default_value_t = false)]
        allow_empty: bool,
    },
    /// Apply a unified diff to a slug's current value (safer than set).
    ///
    /// Reads the diff from stdin or `--patch-file`. Refuses to apply if the
    /// slug has changed since `--base-hash` was captured, and refuses
    /// hunks whose context doesn't match the current value verbatim.
    Patch {
        slug: String,
        /// Read the patch from a file instead of stdin.
        #[arg(long)]
        patch_file: Option<String>,
        /// sha256 hex digest (lowercase) of the value the patch was generated
        /// against. Hashes the exact UTF-8 bytes returned by `buzz mem get`,
        /// not normalized lines. Run `buzz mem hash <slug>` to capture this
        /// before editing.
        #[arg(long)]
        base_hash: Option<String>,
        /// Skip the base-hash check. Unsafe if concurrent edits are possible —
        /// the patch will be applied against whatever the current value is,
        /// even if another agent rewrote it after the patch was generated.
        #[arg(long, default_value_t = false)]
        no_base_hash: bool,
        /// Echo the input patch + resulting sha256 and exit without writing.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        /// Allow committing an empty result.
        #[arg(long, default_value_t = false)]
        allow_empty: bool,
        #[arg(long)]
        owner: Option<String>,
    },
    /// Publish a tombstone for a slug (cannot be used on `core`).
    Rm {
        slug: String,
        #[arg(long)]
        owner: Option<String>,
    },
}

/// Subcommands for `buzz pack`.
#[derive(Subcommand)]
pub enum PackCmd {
    /// Validate a persona pack directory
    Validate {
        /// Path to the pack directory
        path: String,
    },
    /// Inspect a persona pack — show metadata and effective config
    Inspect {
        /// Path to the pack directory
        path: String,
    },
}

/// Community moderation commands.
///
/// The community (tenant) is selected by the relay host in `--relay` /
/// `BUZZ_RELAY_URL` — moderation commands are community-global and carry no
/// channel scope. The signing key must be a community owner/admin; the relay
/// authorizes every command.
#[derive(Subcommand)]
pub enum ModerationCmd {
    /// List reports in the moderation queue (newest first)
    #[command(
        after_help = "Examples:\n  buzz moderation reports\n  buzz moderation reports --status open --limit 20"
    )]
    Reports {
        /// Filter by status: open | resolved | dismissed | escalated (default: all)
        #[arg(long)]
        status: Option<String>,
        /// Maximum number of reports to return
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Resolve or dismiss a report (kind 9044)
    #[command(
        after_help = "Examples:\n  buzz moderation resolve --report <REPORT_EVENT_ID> --status dismissed --action dismiss\n  buzz moderation resolve --report <REPORT_EVENT_ID> --status resolved --action ban --reason \"rule 3\""
    )]
    Resolve {
        /// Hex event id of the kind:1984 report being resolved
        #[arg(long)]
        report: String,
        /// Resolution status: resolved | dismissed
        #[arg(long)]
        status: String,
        /// Action taken: delete | kick | ban | timeout | dismiss | escalate
        #[arg(long)]
        action: String,
        /// Optional reason — relayed to the reporter, so keep it tombstone-safe
        #[arg(long)]
        reason: Option<String>,
    },
    /// Ban a member from the community (kind 9040)
    #[command(
        after_help = "Examples:\n  buzz moderation ban --pubkey <HEX>\n  buzz moderation ban --pubkey <HEX> --expires-in 604800 --reason \"repeated spam\""
    )]
    Ban {
        /// Target member pubkey (hex)
        #[arg(long)]
        pubkey: String,
        /// Ban duration in seconds from now (omit for a permanent ban)
        #[arg(long, conflicts_with = "expires_at")]
        expires_in: Option<u64>,
        /// Absolute ban expiry as a unix timestamp (seconds)
        #[arg(long)]
        expires_at: Option<u64>,
        /// Optional private ban reason (audit only)
        #[arg(long)]
        reason: Option<String>,
    },
    /// Lift a member's ban (kind 9041)
    Unban {
        /// Target member pubkey (hex)
        #[arg(long)]
        pubkey: String,
    },
    /// Time out a member — a write-block, not a disconnect (kind 9042)
    #[command(
        after_help = "Examples:\n  buzz moderation timeout --pubkey <HEX> --expires-in 3600\n  buzz moderation timeout --pubkey <HEX> --expires-at 1783500000 --reason \"cool off\""
    )]
    Timeout {
        /// Target member pubkey (hex)
        #[arg(long)]
        pubkey: String,
        /// Timeout duration in seconds from now
        #[arg(long, conflicts_with = "expires_at")]
        expires_in: Option<u64>,
        /// Absolute timeout expiry as a unix timestamp (seconds)
        #[arg(long)]
        expires_at: Option<u64>,
        /// Optional private timeout reason (audit only)
        #[arg(long)]
        reason: Option<String>,
    },
    /// Clear a member's timeout early (kind 9043)
    Untimeout {
        /// Target member pubkey (hex)
        #[arg(long)]
        pubkey: String,
    },
    /// List currently-restricted members (active ban or timeout)
    Restricted,
    /// Read the moderation audit trail (newest first)
    Audit {
        /// Maximum number of audit rows to return
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
}

/// Normalize hand-authored `BUZZ_AUTH_TAG` input to strict JSON.
///
/// `.env` files and shell exports sometimes carry the tag in the unquoted
/// shorthand `[auth,<hex>,<conditions>,<hex>]` (quotes dropped by hand).
/// When the input is not valid JSON but is bracket-delimited, rewrite it as
/// a JSON array of the comma-separated fields (an empty field `,,` becomes
/// `""`, matching the canonical form `["auth","hex","","hex"]`).
///
/// This is presentation-layer leniency at the configuration edge only: the
/// output is always fed through the SDK's strict `parse_auth_tag` /
/// `verify_auth_tag`, which enforce structure, hex, the conditions grammar,
/// and the BIP-340 signature. Inputs that are already valid JSON — or not
/// recognizable as the shorthand — are returned unchanged so the strict
/// parser reports the error on the original bytes.
fn normalize_auth_tag_input(input: &str) -> String {
    let trimmed = input.trim();
    if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
        return trimmed.to_owned();
    }
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        let fields: Vec<&str> = trimmed[1..trimmed.len() - 1]
            .split(',')
            .map(str::trim)
            .collect();
        // Only a plausible 4-field auth tag is rewritten; anything else is
        // passed through untouched for the strict parser to reject with an
        // error that references the caller's original input.
        if fields.len() == 4 && !fields.iter().any(|f| f.contains('"')) {
            // serde_json cannot fail serializing a Vec<&str>.
            return serde_json::to_string(&fields).expect("string array serializes");
        }
    }
    trimmed.to_owned()
}

async fn run(cli: Cli) -> Result<(), CliError> {
    let relay_url = client::normalize_relay_url(&cli.relay);

    // Pack commands are local-only — no relay connection needed.
    if let Cmd::Pack(ref sub) = cli.command {
        return match sub {
            PackCmd::Validate { path } => commands::pack::cmd_validate(path),
            PackCmd::Inspect { path } => commands::pack::cmd_inspect(path),
        };
    }

    // Auth: private key is required for all relay operations.
    // The keypair IS the identity — no tokens, no other auth.
    let private_key_str = cli.private_key.ok_or_else(|| {
        CliError::Auth("BUZZ_PRIVATE_KEY is required (use --private-key or set env var)".into())
    })?;
    let keys = Keys::parse(&private_key_str)
        .map_err(|e| CliError::Key(format!("invalid BUZZ_PRIVATE_KEY: {e}")))?;

    // NIP-OA: parse and verify the auth tag if provided.
    //
    // `BUZZ_AUTH_TAG` is hand-authored configuration, so the unquoted raw
    // shorthand `[auth,hex,,hex]` is normalized to JSON here — at this input
    // edge only. The SDK grammar and the `x-auth-tag` wire format stay strict
    // JSON; all validation and signature verification happen on the strict
    // path below, unchanged.
    let (auth_tag, auth_tag_json) = match cli.auth_tag {
        Some(ref input) if !input.is_empty() => {
            let json = normalize_auth_tag_input(input);
            let tag = buzz_sdk::nip_oa::parse_auth_tag(&json)
                .map_err(|e| CliError::Auth(format!("BUZZ_AUTH_TAG is malformed: {e}")))?;
            buzz_sdk::nip_oa::verify_auth_tag(&json, &keys.public_key()).map_err(|e| {
                CliError::Auth(format!(
                    "BUZZ_AUTH_TAG verification failed for pubkey {}: {e}",
                    keys.public_key().to_hex()
                ))
            })?;
            // Canonical wire form derives from the parsed-and-verified tag
            // (same shape as buzz-acp's RestClient), never from raw input.
            let canonical = serde_json::to_string(tag.as_slice())
                .map_err(|e| CliError::Auth(format!("BUZZ_AUTH_TAG serialization failed: {e}")))?;
            (Some(tag), Some(canonical))
        }
        _ => (None, None),
    };

    let client = BuzzClient::new(relay_url, keys, auth_tag, auth_tag_json)?;

    match cli.command {
        Cmd::Agents(sub) => commands::agents::dispatch(sub, &client).await,
        Cmd::Messages(sub) => commands::messages::dispatch(sub, &client, &cli.format).await,
        Cmd::Channels(sub) => commands::channels::dispatch(sub, &client, &cli.format).await,
        Cmd::Canvas(sub) => commands::channels::dispatch_canvas(sub, &client).await,
        Cmd::Reactions(sub) => commands::reactions::dispatch(sub, &client).await,
        Cmd::Emoji(sub) => commands::emoji::dispatch(sub, &client).await,
        Cmd::Dms(sub) => commands::dms::dispatch(sub, &client).await,
        Cmd::Users(sub) => commands::users::dispatch(sub, &client, &cli.format).await,
        Cmd::Workflows(sub) => commands::workflows::dispatch(sub, &client).await,
        Cmd::Feed(sub) => commands::feed::dispatch(sub, &client, &cli.format).await,
        Cmd::Social(sub) => commands::social::dispatch(sub, &client).await,
        Cmd::Notes(sub) => commands::notes::dispatch(sub, &client).await,
        Cmd::Repos(sub) => commands::repos::dispatch(sub, &client).await,
        Cmd::Projects(sub) => commands::projects::dispatch(sub, &client).await,
        Cmd::Patches(sub) => commands::patches::dispatch(sub, &client).await,
        Cmd::Issues(sub) => commands::issues::dispatch(sub, &client).await,
        Cmd::Pr(sub) => commands::pr::dispatch(sub, &client).await,
        Cmd::Media(sub) => commands::upload::dispatch_media(sub, &client).await,
        Cmd::Upload(sub) => commands::upload::dispatch(sub, &client).await,
        Cmd::Sandbox(sub) => commands::sandbox::dispatch(sub, &client).await,
        Cmd::Mem(sub) => commands::mem::dispatch(sub, &client).await,
        Cmd::Moderation(sub) => commands::moderation::dispatch(sub, &client, &cli.format).await,
        Cmd::Skills(sub) => commands::skills::dispatch(sub, &client).await,
        Cmd::Pack(_) => unreachable!("handled above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// Raw shorthand `[auth,hex,,hex]` normalizes to strict JSON; the empty
    /// conditions field becomes `""`.
    #[test]
    fn normalize_auth_tag_raw_shorthand() {
        let owner = "a".repeat(64);
        let sig = "b".repeat(128);

        let raw = format!("[auth,{owner},,{sig}]");
        let json = normalize_auth_tag_input(&raw);
        let parsed: Vec<String> = serde_json::from_str(&json).expect("output must be JSON");
        assert_eq!(parsed, vec!["auth", &owner, "", &sig]);

        // With conditions and surrounding whitespace (shell/.env artifacts).
        let raw = format!("  [auth, {owner} , kind=9, {sig}]  \n");
        let json = normalize_auth_tag_input(&raw);
        let parsed: Vec<String> = serde_json::from_str(&json).expect("output must be JSON");
        assert_eq!(parsed, vec!["auth", &owner, "kind=9", &sig]);
    }

    /// Valid JSON input passes through byte-identical (modulo outer trim) —
    /// the normalizer must never rewrite well-formed input.
    #[test]
    fn normalize_auth_tag_json_passthrough() {
        let owner = "a".repeat(64);
        let sig = "b".repeat(128);
        let json_in = serde_json::json!(["auth", owner, "kind=9", sig]).to_string();
        assert_eq!(normalize_auth_tag_input(&json_in), json_in);
    }

    /// Inputs that are neither JSON nor a plausible 4-field shorthand pass
    /// through unchanged, so the strict parser rejects the original bytes.
    #[test]
    fn normalize_auth_tag_leaves_garbage_untouched() {
        for garbage in [
            "not a tag",
            "[auth,too,few]",
            "[a,b,c,d,e]",
            r#"[auth,"quoted",x,y]"#, // quote chars => not the shorthand
            "[]",
            "{\"auth\":1}",
        ] {
            assert_eq!(normalize_auth_tag_input(garbage), garbage.trim());
        }
    }

    /// Smoke test: CLI definition is valid and parseable.
    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn messages_thread_accepts_link_or_explicit_identifiers() {
        let channel = "123e4567-e89b-12d3-a456-426614174000";
        let event = "a".repeat(64);
        let link = format!("buzz://message?channel={channel}&id={event}");

        assert!(
            Cli::try_parse_from(["buzz", "messages", "thread", "--link", link.as_str(),]).is_ok()
        );
        assert!(Cli::try_parse_from([
            "buzz",
            "messages",
            "thread",
            "--channel",
            channel,
            "--event",
            event.as_str(),
        ])
        .is_ok());
    }

    #[test]
    fn messages_thread_rejects_partial_or_mixed_targets() {
        let channel = "123e4567-e89b-12d3-a456-426614174000";
        let event = "a".repeat(64);
        let link = format!("buzz://message?channel={channel}&id={event}");

        assert!(Cli::try_parse_from(["buzz", "messages", "thread"]).is_err());
        assert!(Cli::try_parse_from(["buzz", "messages", "thread", "--channel", channel]).is_err());
        assert!(Cli::try_parse_from([
            "buzz",
            "messages",
            "thread",
            "--link",
            link.as_str(),
            "--event",
            event.as_str(),
        ])
        .is_err());
    }

    #[test]
    fn set_status_clear_rejects_text_and_emoji() {
        for extra in [["--text", "busy"], ["--emoji", "🎶"]] {
            let args = ["buzz", "users", "set-status", "--clear"]
                .into_iter()
                .chain(extra);
            assert!(
                Cli::try_parse_from(args).is_err(),
                "--clear must conflict with {}",
                extra[0]
            );
        }
    }

    #[test]
    fn set_status_requires_text_or_clear() {
        assert!(Cli::try_parse_from(["buzz", "users", "set-status"]).is_err());
        assert!(
            Cli::try_parse_from(["buzz", "users", "set-status", "--emoji", "🎶"]).is_err(),
            "--emoji alone must not imply a status"
        );
        assert!(Cli::try_parse_from(["buzz", "users", "set-status", "--clear"]).is_ok());
    }

    #[test]
    fn command_inventory_is_stable() {
        let expected_groups: Vec<&str> = vec![
            "agents",
            "canvas",
            "channels",
            "dms",
            "emoji",
            "feed",
            "issues",
            "media",
            "mem",
            "messages",
            "moderation",
            "notes",
            "pack",
            "patches",
            "pr",
            "projects",
            "reactions",
            "repos",
            "sandbox",
            "skills",
            "social",
            "upload",
            "users",
            "workflows",
        ];

        let cmd = Cli::command();
        let mut actual: Vec<String> = cmd
            .get_subcommands()
            .map(|s| s.get_name().to_string())
            .filter(|n| n != "help")
            .collect();
        actual.sort();

        assert_eq!(
            actual.len(),
            expected_groups.len(),
            "Expected {} groups, got {}. Actual: {:?}",
            expected_groups.len(),
            actual.len(),
            actual
        );
        assert_eq!(
            actual, expected_groups,
            "Command group inventory drift detected"
        );
    }

    #[test]
    fn subcommand_names_are_stable() {
        fn names(cmd: &clap::Command, group: &str) -> Vec<String> {
            let group_cmd = cmd
                .get_subcommands()
                .find(|s| s.get_name() == group)
                .unwrap_or_else(|| panic!("group '{}' not found", group));
            let mut names: Vec<String> = group_cmd
                .get_subcommands()
                .map(|s| s.get_name().to_string())
                .filter(|n| n != "help")
                .collect();
            names.sort();
            names
        }

        let cmd = Cli::command();
        assert_eq!(
            names(&cmd, "agents"),
            vec![
                "archive",
                "archived",
                "draft-create",
                "draft-update",
                "unarchive"
            ]
        );
        assert_eq!(
            names(&cmd, "messages"),
            vec![
                "delete",
                "edit",
                "get",
                "search",
                "send",
                "send-diff",
                "thread",
                "vote"
            ]
        );
        assert_eq!(
            names(&cmd, "channels"),
            vec![
                "add-member",
                "archive",
                "create",
                "delete",
                "get",
                "join",
                "leave",
                "list",
                "members",
                "purpose",
                "remove-member",
                "search",
                "set-add-policy",
                "topic",
                "unarchive",
                "update"
            ]
        );
        assert_eq!(names(&cmd, "canvas"), vec!["get", "set"]);
        assert_eq!(names(&cmd, "reactions"), vec!["add", "get", "remove"]);
        assert_eq!(
            names(&cmd, "emoji"),
            vec!["export", "import", "list", "rm", "set"]
        );
        assert_eq!(
            names(&cmd, "dms"),
            vec!["add-member", "hide", "list", "open"]
        );
        assert_eq!(
            names(&cmd, "users"),
            vec![
                "get",
                "presence",
                "set-presence",
                "set-profile",
                "set-status"
            ]
        );
        assert_eq!(
            names(&cmd, "workflows"),
            vec!["approve", "create", "delete", "get", "list", "runs", "trigger", "update"]
        );
        assert_eq!(names(&cmd, "feed"), vec!["get"]);
        assert_eq!(
            names(&cmd, "social"),
            vec![
                "contacts",
                "event",
                "list",
                "notes",
                "publish",
                "set-contacts",
                "set-list"
            ]
        );
        assert_eq!(
            names(&cmd, "repos"),
            vec!["bind", "create", "get", "list", "protect"]
        );
        let repos = cmd
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "repos")
            .expect("repos command");
        let protect = repos
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "protect")
            .expect("repos protect command");
        let mut protect_names: Vec<String> = protect
            .get_subcommands()
            .map(|subcommand| subcommand.get_name().to_string())
            .filter(|name| name != "help")
            .collect();
        protect_names.sort();
        assert_eq!(protect_names, vec!["list", "remove", "set"]);
        assert_eq!(
            names(&cmd, "pr"),
            vec!["get", "list", "open", "status", "update"]
        );
        assert_eq!(
            names(&cmd, "patches"),
            vec!["get", "list", "send", "status"]
        );
        assert_eq!(
            names(&cmd, "projects"),
            vec![
                "add-channel",
                "add-repo",
                "create",
                "delete",
                "get",
                "list",
                "remove-repo",
                "update"
            ]
        );
        assert_eq!(
            names(&cmd, "issues"),
            vec!["assign", "create", "get", "list", "status", "unassign"]
        );
        assert_eq!(names(&cmd, "media"), vec!["get"]);
        assert_eq!(names(&cmd, "upload"), vec!["file"]);
        assert_eq!(names(&cmd, "pack"), vec!["inspect", "validate"]);
        assert_eq!(
            names(&cmd, "moderation"),
            vec![
                "audit",
                "ban",
                "reports",
                "resolve",
                "restricted",
                "timeout",
                "unban",
                "untimeout"
            ]
        );
    }

    #[test]
    fn subcommand_counts_are_stable() {
        let expected: Vec<(&str, usize)> = vec![
            ("agents", 5),
            ("canvas", 2),
            ("channels", 16),
            ("dms", 4),
            ("emoji", 5),
            ("feed", 1),
            ("issues", 6),
            ("media", 1),
            ("messages", 8),
            ("pack", 2),
            ("patches", 4),
            ("pr", 5),
            ("projects", 8),
            ("reactions", 3),
            ("repos", 5),
            ("social", 7),
            ("upload", 1),
            ("users", 5),
            ("workflows", 8),
        ];

        let cmd = Cli::command();
        for (group_name, expected_count) in &expected {
            let group = cmd
                .get_subcommands()
                .find(|s| s.get_name() == *group_name)
                .unwrap_or_else(|| panic!("group '{}' not found", group_name));
            let actual_count = group
                .get_subcommands()
                .filter(|s| s.get_name() != "help")
                .count();
            assert_eq!(
                actual_count, *expected_count,
                "Group '{}': expected {} subcommands, got {}",
                group_name, expected_count, actual_count
            );
        }
    }

    /// Collect all args (recursing into subcommands) whose env var name looks
    /// like a credential but does NOT have `hide_env_values` set.
    fn collect_unhidden_secret_args(cmd: &clap::Command) -> Vec<(String, String)> {
        const SECRET_PATTERNS: &[&str] = &["KEY", "SECRET", "TOKEN", "PASSWORD", "CRED", "AUTH"];

        let mut violations: Vec<(String, String)> = Vec::new();

        for arg in cmd.get_arguments() {
            if let Some(env_key) = arg.get_env() {
                let env_name = env_key.to_string_lossy().to_uppercase();
                let is_secret = SECRET_PATTERNS.iter().any(|pat| env_name.contains(pat));
                if is_secret && !arg.is_hide_env_values_set() {
                    violations.push((cmd.get_name().to_string(), env_name));
                }
            }
        }

        for sub in cmd.get_subcommands() {
            violations.extend(collect_unhidden_secret_args(sub));
        }

        violations
    }

    /// Every arg whose env var name contains KEY/SECRET/TOKEN/PASSWORD/CRED/AUTH
    /// must set `hide_env_values = true` to prevent credential leakage in --help.
    #[test]
    fn secret_env_args_hide_their_values_in_help() {
        let cmd = Cli::command();
        let violations = collect_unhidden_secret_args(&cmd);
        assert!(
            violations.is_empty(),
            "Found secret-bearing env args without hide_env_values=true. \
             Add `hide_env_values = true` to each:\n{}",
            violations
                .iter()
                .map(|(cmd, env)| format!("  command={cmd:?} env={env:?}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    // ── projects update mutation group ────────────────────────────────────────

    /// Project-channel requests accept the owner-review metadata.
    #[test]
    fn projects_add_channel_accepts_owner_review_fields() {
        assert!(Cli::try_parse_from([
            "buzz",
            "projects",
            "add-channel",
            "--home-channel",
            "11111111-1111-4111-8111-111111111111",
            "--name",
            "release-planning",
            "--visibility",
            "private",
            "--template",
            "Release team",
        ])
        .is_ok());
    }

    /// Multiple independent fields must be accepted in the same invocation.
    #[test]
    fn projects_update_multi_field_is_accepted() {
        assert!(
            Cli::try_parse_from([
                "buzz",
                "projects",
                "update",
                "my-slug",
                "--name",
                "X",
                "--description",
                "Y",
            ])
            .is_ok(),
            "--name and --description together must be accepted"
        );
    }

    /// A setter for one field and a clearer for a different field must be accepted.
    #[test]
    fn projects_update_setter_with_other_clearer_is_accepted() {
        assert!(
            Cli::try_parse_from([
                "buzz",
                "projects",
                "update",
                "my-slug",
                "--name",
                "X",
                "--clear-description",
            ])
            .is_ok(),
            "--name with --clear-description must be accepted"
        );
    }

    /// A setter and its own clearer are mutually exclusive — clap must reject this.
    #[test]
    fn projects_update_setter_with_own_clearer_is_rejected() {
        assert!(
            Cli::try_parse_from([
                "buzz",
                "projects",
                "update",
                "my-slug",
                "--name",
                "X",
                "--clear-name",
            ])
            .is_err(),
            "--name and --clear-name together must be rejected by clap"
        );
    }

    /// Providing no mutation options at all must be rejected by clap (required group).
    #[test]
    fn projects_update_no_mutation_is_rejected_by_clap() {
        // Without credentials, a valid parse would reach authentication and fail
        // with auth_error — but a clap-level rejection happens before any I/O.
        // We verify it's a clap error (not just any error) by checking the error
        // kind is not a runtime/auth failure — Cli::try_parse_from returns Err
        // immediately for argument violations.
        assert!(
            Cli::try_parse_from(["buzz", "projects", "update", "my-slug"]).is_err(),
            "update with no setters or clearers must be rejected at parse time"
        );
    }

    /// An unrecognised visibility token must be rejected by clap before any I/O.
    #[test]
    fn projects_create_invalid_visibility_is_rejected_by_clap() {
        assert!(
            Cli::try_parse_from([
                "buzz",
                "projects",
                "create",
                "my-slug",
                "--repo",
                "buzz",
                "--visibility",
                "chartreuse",
            ])
            .is_err(),
            "--visibility chartreuse must be rejected at parse time"
        );
    }

    /// An unrecognised visibility token on update must be rejected by clap before any I/O.
    #[test]
    fn projects_update_invalid_visibility_is_rejected_by_clap() {
        assert!(
            Cli::try_parse_from([
                "buzz",
                "projects",
                "update",
                "my-slug",
                "--visibility",
                "chartreuse",
            ])
            .is_err(),
            "--visibility chartreuse on update must be rejected at parse time"
        );
    }

    // ── sandbox computer-use commands ──────────────────────────────────────

    /// With no positional id, no BUZZ_SANDBOX_ID, and not running inside a
    /// container, resolution must return a Usage error naming both the
    /// positional arg and the env var — this is the only way an agent
    /// without an id learns what to set.
    #[test]
    fn sandbox_id_missing_is_usage_error_naming_arg_and_env_var() {
        let err = resolve_sandbox_id(None, false, || None)
            .expect_err("no id, no env, not in a container must fail");
        match err {
            CliError::Usage(msg) => {
                assert!(
                    msg.contains("argument"),
                    "usage error must mention the positional argument: {msg}"
                );
                assert!(
                    msg.contains("BUZZ_SANDBOX_ID"),
                    "usage error must name BUZZ_SANDBOX_ID: {msg}"
                );
                assert!(
                    msg.contains("auto-detected"),
                    "usage error must mention that a Buzz computer auto-detects its id: {msg}"
                );
            }
            other => panic!("expected Usage error, got {other:?}"),
        }
    }

    /// Inside a container but with no plausible container-id hostname
    /// (covers both a non-hex hostname and a hostname read failure), the
    /// error must still be a Usage error — self-detection never invents an
    /// id when the guard doesn't clear.
    #[test]
    fn sandbox_id_in_container_with_bad_hostname_falls_through_to_usage_error() {
        for hostname in [
            Some("not-hex-at-all".to_string()),
            Some("ab12".to_string()),
            None,
        ] {
            let err = resolve_sandbox_id(None, true, || hostname.clone())
                .expect_err("non-hex or too-short hostname must not resolve");
            assert!(matches!(err, CliError::Usage(_)));
        }
    }

    /// The explicit positional always wins, even inside a container with a
    /// plausible hostname — precedence order 1 beats order 3.
    #[test]
    fn sandbox_id_explicit_beats_self_detection() {
        let id = resolve_sandbox_id(Some("explicit-id"), true, || {
            Some("abcdef012345".to_string())
        })
        .expect("explicit id must resolve");
        assert_eq!(id, "explicit-id");
    }

    #[test]
    fn sandbox_create_leaves_self_service_memory_to_request_builder() {
        let Cmd::Sandbox(SandboxCmd::Create { memory_mb, .. }) = Cli::try_parse_from([
            "buzz",
            "--relay",
            "wss://x",
            "sandbox",
            "create",
            "--broker",
            "http://broker",
        ])
        .expect("sandbox create parses with defaults")
        .command
        else {
            panic!("expected Sandbox(Create)");
        };
        assert_eq!(memory_mb, None);
    }

    #[test]
    fn sandbox_create_preserves_explicit_memory_override() {
        let Cmd::Sandbox(SandboxCmd::Create { memory_mb, .. }) = Cli::try_parse_from([
            "buzz",
            "--relay",
            "wss://x",
            "sandbox",
            "create",
            "--broker",
            "http://broker",
            "--memory-mb",
            "2048",
        ])
        .expect("sandbox create parses with an explicit memory limit")
        .command
        else {
            panic!("expected Sandbox(Create)");
        };
        assert_eq!(memory_mb, Some(2048));
    }

    /// `BUZZ_SANDBOX_ID` is modeled as clap's `env` fallback landing in
    /// `explicit` before `require()` ever runs (clap resolves positional-or-env
    /// before the app sees the value) — so env-set beats self-detection too,
    /// via the same `explicit` branch.
    #[test]
    fn sandbox_id_env_fallback_beats_self_detection() {
        let id = resolve_sandbox_id(Some("from-env"), true, || Some("abcdef012345".to_string()))
            .expect("env-sourced id must resolve");
        assert_eq!(id, "from-env");
    }

    /// With no explicit id but inside a container with a hostname that looks
    /// like a Docker short container id (12+ hex chars), self-detection
    /// resolves it — this is the whole point of the fallback: the broker
    /// can't inject BUZZ_SANDBOX_ID at container-create time.
    #[test]
    fn sandbox_id_self_detection_uses_hex_hostname_in_container() {
        let id = resolve_sandbox_id(None, true, || Some("a1b2c3d4e5f6".to_string()))
            .expect("12-hex-char hostname in a container must resolve");
        assert_eq!(id, "a1b2c3d4e5f6");
    }

    /// A hostname that happens to satisfy the hex-shape guard but has mixed
    /// case is normalized to lowercase — Docker ids are conventionally
    /// lowercase and the broker should see a consistent form.
    #[test]
    fn sandbox_id_self_detection_normalizes_case() {
        let id = resolve_sandbox_id(None, true, || Some("A1B2C3D4E5F6".to_string()))
            .expect("uppercase-hex hostname must still resolve");
        assert_eq!(id, "a1b2c3d4e5f6");
    }

    /// Outside a container (the common case: developer's own machine, or the
    /// broker host itself), self-detection must never fire even if a fake
    /// hostname source would satisfy the hex guard — `in_container` is the
    /// gate, not just the hostname shape.
    #[test]
    fn sandbox_id_self_detection_skipped_outside_container() {
        let err = resolve_sandbox_id(None, false, || Some("a1b2c3d4e5f6".to_string()))
            .expect_err("hex-looking hostname outside a container must not resolve");
        assert!(matches!(err, CliError::Usage(_)));
    }

    #[test]
    fn container_id_from_hostname_accepts_twelve_hex_chars() {
        assert_eq!(
            container_id_from_hostname("a1b2c3d4e5f6"),
            Some("a1b2c3d4e5f6".to_string())
        );
    }

    #[test]
    fn container_id_from_hostname_accepts_longer_hex_ids() {
        assert_eq!(
            container_id_from_hostname("a1b2c3d4e5f678901234"),
            Some("a1b2c3d4e5f678901234".to_string())
        );
    }

    #[test]
    fn container_id_from_hostname_accepts_sixty_four_hex_chars() {
        let id64 = "a".repeat(64);
        assert_eq!(container_id_from_hostname(&id64), Some(id64));
    }

    #[test]
    fn container_id_from_hostname_rejects_over_sixty_four_hex_chars() {
        let id65 = "a".repeat(65);
        assert_eq!(container_id_from_hostname(&id65), None);
    }

    #[test]
    fn container_id_from_hostname_rejects_short_hex() {
        assert_eq!(container_id_from_hostname("a1b2c3d4e5f"), None);
    }

    #[test]
    fn container_id_from_hostname_rejects_non_hex() {
        for bad in ["localhost", "my-laptop", "sandbox-01", ""] {
            assert_eq!(container_id_from_hostname(bad), None, "input: {bad:?}");
        }
    }

    /// An explicit positional id takes precedence and resolves cleanly.
    #[test]
    fn sandbox_id_explicit_positional_resolves() {
        let Cmd::Sandbox(SandboxCmd::Click { sandbox, .. }) = Cli::try_parse_from([
            "buzz",
            "--relay",
            "wss://x",
            "sandbox",
            "click",
            "--broker",
            "http://broker",
            "1",
            "2",
            "explicit-id",
        ])
        .expect("parses with an explicit id")
        .command
        else {
            panic!("expected Sandbox(Click)");
        };
        assert_eq!(sandbox.require().unwrap(), "explicit-id");
    }

    /// `exec` accepts trailing argv without `--command`.
    #[test]
    fn sandbox_exec_accepts_trailing_argv() {
        let Cmd::Sandbox(SandboxCmd::Exec { command, argv, .. }) = Cli::try_parse_from([
            "buzz",
            "--relay",
            "wss://x",
            "sandbox",
            "exec",
            "--broker",
            "http://broker",
            "myid",
            "--",
            "ls",
            "-la",
            "/workspace",
        ])
        .expect("parses with trailing argv")
        .command
        else {
            panic!("expected Sandbox(Exec)");
        };
        assert!(command.is_none());
        assert_eq!(argv, vec!["ls", "-la", "/workspace"]);
    }

    /// `exec -c "<string>"` and trailing argv are mutually exclusive —
    /// clap must reject the combination before dispatch ever sees it.
    #[test]
    fn sandbox_exec_command_and_argv_are_mutually_exclusive() {
        assert!(
            Cli::try_parse_from([
                "buzz",
                "--relay",
                "wss://x",
                "sandbox",
                "exec",
                "--broker",
                "http://broker",
                "myid",
                "-c",
                "echo hi",
                "--",
                "ls",
            ])
            .is_err(),
            "-c and trailing argv together must be rejected at parse time"
        );
    }

    /// `exec -c` alone parses to a `command` value with empty argv.
    #[test]
    fn sandbox_exec_command_flag_parses() {
        let Cmd::Sandbox(SandboxCmd::Exec { command, argv, .. }) = Cli::try_parse_from([
            "buzz",
            "--relay",
            "wss://x",
            "sandbox",
            "exec",
            "--broker",
            "http://broker",
            "myid",
            "-c",
            "echo hi",
        ])
        .expect("parses with -c")
        .command
        else {
            panic!("expected Sandbox(Exec)");
        };
        assert_eq!(command.as_deref(), Some("echo hi"));
        assert!(argv.is_empty());
    }

    /// `exec` with neither `-c` nor trailing argv parses fine at the clap
    /// layer (both are optional) but must be rejected by dispatch — model
    /// that mapping directly against the same argv/command shape dispatch
    /// sees, without needing a live broker.
    #[test]
    fn sandbox_exec_requires_command_or_argv_at_dispatch() {
        let Cmd::Sandbox(SandboxCmd::Exec { command, argv, .. }) = Cli::try_parse_from([
            "buzz",
            "--relay",
            "wss://x",
            "sandbox",
            "exec",
            "--broker",
            "http://broker",
            "myid",
        ])
        .expect("parses with neither -c nor argv")
        .command
        else {
            panic!("expected Sandbox(Exec)");
        };
        assert!(command.is_none());
        assert!(argv.is_empty());
        // Mirrors commands::sandbox::dispatch's argv resolution.
        let resolved = match (command, argv.is_empty()) {
            (Some(_), true) => unreachable!(),
            (None, false) => unreachable!(),
            (Some(_), false) => unreachable!(),
            (None, true) => Err::<Vec<String>, _>(CliError::Usage(
                "exec requires either --command <string> or trailing arguments after --"
                    .to_string(),
            )),
        };
        assert!(matches!(resolved, Err(CliError::Usage(_))));
    }

    /// Action-producing commands (click, key, scroll, ...) must map their
    /// flags into the exact broker action shape the contract defines.
    #[test]
    fn sandbox_click_action_body_shape() {
        let Cmd::Sandbox(SandboxCmd::Click {
            x,
            y,
            button,
            sandbox,
            ..
        }) = Cli::try_parse_from([
            "buzz",
            "--relay",
            "wss://x",
            "sandbox",
            "click",
            "--broker",
            "http://broker",
            "10",
            "20",
            "myid",
            "--button",
            "right",
        ])
        .expect("parses")
        .command
        else {
            panic!("expected Sandbox(Click)");
        };
        assert_eq!(sandbox.require().unwrap(), "myid");
        let action = serde_json::json!([{ "type": "click", "x": x, "y": y, "button": button }]);
        assert_eq!(
            action,
            serde_json::json!([{ "type": "click", "x": 10, "y": 20, "button": "right" }])
        );
    }

    #[test]
    fn sandbox_scroll_action_body_shape() {
        let Cmd::Sandbox(SandboxCmd::Scroll {
            x,
            y,
            direction,
            amount,
            ..
        }) = Cli::try_parse_from([
            "buzz",
            "--relay",
            "wss://x",
            "sandbox",
            "scroll",
            "--broker",
            "http://broker",
            "5",
            "6",
            "myid",
            "--direction",
            "down",
            "--amount",
            "3",
        ])
        .expect("parses")
        .command
        else {
            panic!("expected Sandbox(Scroll)");
        };
        let action = serde_json::json!([{
            "type": "scroll",
            "x": x,
            "y": y,
            "direction": direction,
            "amount": amount,
        }]);
        assert_eq!(
            action,
            serde_json::json!([{ "type": "scroll", "x": 5, "y": 6, "direction": "down", "amount": 3 }])
        );
    }

    /// `open --app browser --url ...` maps into the launch request shape.
    #[test]
    fn sandbox_open_launch_body_shape() {
        let Cmd::Sandbox(SandboxCmd::Open { app, url, .. }) = Cli::try_parse_from([
            "buzz",
            "--relay",
            "wss://x",
            "sandbox",
            "open",
            "--broker",
            "http://broker",
            "myid",
            "--app",
            "browser",
            "--url",
            "https://example.com",
        ])
        .expect("parses")
        .command
        else {
            panic!("expected Sandbox(Open)");
        };
        let mut body = serde_json::json!({ "app": app });
        if let Some(url) = url {
            body["url"] = serde_json::json!(url);
        }
        assert_eq!(
            body,
            serde_json::json!({ "app": "browser", "url": "https://example.com" })
        );
    }

    #[test]
    fn sandbox_files_list_parses_remote_directory_and_id() {
        let Cmd::Sandbox(SandboxCmd::Files {
            command: SandboxFilesCmd::List { path, sandbox, .. },
        }) = Cli::try_parse_from([
            "buzz",
            "--relay",
            "wss://x",
            "sandbox",
            "files",
            "list",
            "--broker",
            "http://broker",
            "myid",
            "--path",
            "/home/agent/Downloads",
        ])
        .expect("files list parses")
        .command
        else {
            panic!("expected Sandbox(Files(List))");
        };
        assert_eq!(sandbox.require().unwrap(), "myid");
        assert_eq!(path, "/home/agent/Downloads");
    }

    #[test]
    fn sandbox_files_get_parses_paths_and_id() {
        let Cmd::Sandbox(SandboxCmd::Files {
            command:
                SandboxFilesCmd::Get {
                    remote_path,
                    sandbox,
                    output,
                    ..
                },
        }) = Cli::try_parse_from([
            "buzz",
            "--relay",
            "wss://x",
            "sandbox",
            "files",
            "get",
            "--broker",
            "http://broker",
            "/home/agent/Downloads/model.glb",
            "myid",
            "--output",
            "./assets/model.glb",
        ])
        .expect("files get parses")
        .command
        else {
            panic!("expected Sandbox(Files(Get))");
        };
        assert_eq!(sandbox.require().unwrap(), "myid");
        assert_eq!(remote_path, "/home/agent/Downloads/model.glb");
        assert_eq!(output.as_deref(), Some("./assets/model.glb"));
    }

    #[test]
    fn sandbox_files_put_parses_both_paths_and_id() {
        let Cmd::Sandbox(SandboxCmd::Files {
            command:
                SandboxFilesCmd::Put {
                    local_path,
                    remote_path,
                    sandbox,
                    ..
                },
        }) = Cli::try_parse_from([
            "buzz",
            "--relay",
            "wss://x",
            "sandbox",
            "files",
            "put",
            "--broker",
            "http://broker",
            "./assets/model.glb",
            "/workspace/model.glb",
            "myid",
        ])
        .expect("files put parses")
        .command
        else {
            panic!("expected Sandbox(Files(Put))");
        };
        assert_eq!(sandbox.require().unwrap(), "myid");
        assert_eq!(local_path, "./assets/model.glb");
        assert_eq!(remote_path, "/workspace/model.glb");
    }
}

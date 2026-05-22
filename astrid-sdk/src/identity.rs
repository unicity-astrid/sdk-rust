//! Multi-platform identity resolve / link / list-links.
//!
//! Maps external platform identities (Discord user, GitHub user, etc.)
//! to internal Astrid user IDs. Backed by the typed
//! `astrid:identity/host@1.0.0` package. The host returns typed
//! `error-code` variants; this wrapper translates `link-not-found` on
//! resolve into `Ok(None)` so calling code uses idiomatic Rust
//! option semantics for the common "is this user linked?" branch.

use super::*;

/// A resolved Astrid user returned by [`resolve`].
#[derive(Debug, Clone)]
pub struct ResolvedUser {
    /// The Astrid-native user ID (UUID).
    pub user_id: String,
    /// Optional display name.
    pub display_name: Option<String>,
}

/// A platform-to-Astrid identity link.
///
/// `astrid_user_id` is the identity this link belongs to; it's filled
/// in by the SDK from the [`list_links`] caller argument (the WIT
/// `platform-link` record does not carry it since the call already
/// scopes to a single user).
#[derive(Debug, Clone)]
pub struct Link {
    /// Platform name (e.g. "discord", "twitch").
    pub platform: String,
    /// Platform-specific user identifier.
    pub platform_user_id: String,
    /// The Astrid user this is linked to.
    pub astrid_user_id: String,
    /// When the link was created (RFC 3339).
    pub linked_at: String,
    /// How the link was established (e.g. "system", "chat_command").
    pub method: String,
}

/// Resolve a platform user to an Astrid user.
///
/// Returns `Ok(Some(user))` if the platform identity is linked,
/// `Ok(None)` if no link exists. Requires `identity = ["resolve"]` or
/// higher.
pub fn resolve(
    platform: &str,
    platform_user_id: &str,
) -> Result<Option<ResolvedUser>, SysError> {
    let request = wit_identity::IdentityResolveRequest {
        platform: platform.to_string(),
        platform_user_id: platform_user_id.to_string(),
    };
    match wit_identity::identity_resolve(&request) {
        Ok(resp) => Ok(Some(ResolvedUser {
            user_id: resp.user_id,
            display_name: resp.display_name,
        })),
        // The new contract surfaces "no such link" as a typed error
        // variant rather than the pre-migration found:false flag.
        // Treat it as a not-found, not an error.
        Err(wit_identity::ErrorCode::LinkNotFound) => Ok(None),
        Err(e) => Err(host_err(e)),
    }
}

/// Link a platform identity to an Astrid user.
///
/// - `method` describes how the link was established (e.g. "chat_command", "system").
///
/// Requires `identity = ["link"]` or higher. Returns an error with the
/// host-side `AlreadyLinked` variant in the message if this
/// (platform, platform_user_id) pair is already linked.
pub fn link(
    platform: &str,
    platform_user_id: &str,
    astrid_user_id: &str,
    method: &str,
) -> Result<(), SysError> {
    let request = wit_identity::IdentityLinkRequest {
        platform: platform.to_string(),
        platform_user_id: platform_user_id.to_string(),
        astrid_user_id: astrid_user_id.to_string(),
        method: method.to_string(),
    };
    wit_identity::identity_link(&request).map_err(host_err)
}

/// Unlink a platform identity from its Astrid user.
///
/// Returns `Ok(true)` if a link was removed, `Ok(false)` if no link
/// existed. Other failures (capability, store-unavailable, etc.) are
/// returned as `Err`. Requires `identity = ["link"]` or higher.
pub fn unlink(platform: &str, platform_user_id: &str) -> Result<bool, SysError> {
    let request = wit_identity::IdentityUnlinkRequest {
        platform: platform.to_string(),
        platform_user_id: platform_user_id.to_string(),
    };
    match wit_identity::identity_unlink(&request) {
        Ok(()) => Ok(true),
        Err(wit_identity::ErrorCode::LinkNotFound) => Ok(false),
        Err(e) => Err(host_err(e)),
    }
}

/// Create a new Astrid user.
///
/// Returns the UUID of the newly created user. Requires
/// `identity = ["admin"]`.
pub fn create_user(display_name: Option<&str>) -> Result<String, SysError> {
    let request = wit_identity::IdentityCreateUserRequest {
        display_name: display_name.map(str::to_string),
    };
    let resp = wit_identity::identity_create_user(&request).map_err(host_err)?;
    Ok(resp.user_id)
}

/// List all platform links for an Astrid user.
///
/// Returns the (possibly empty) list of linked platform identities for
/// the given user UUID. Requires `identity = ["link"]` or higher.
pub fn list_links(astrid_user_id: &str) -> Result<Vec<Link>, SysError> {
    let links = wit_identity::identity_list_links(astrid_user_id).map_err(host_err)?;
    Ok(links
        .into_iter()
        .map(|l| Link {
            platform: l.platform,
            platform_user_id: l.platform_user_id,
            astrid_user_id: astrid_user_id.to_string(),
            linked_at: l.linked_at,
            method: l.method,
        })
        .collect())
}

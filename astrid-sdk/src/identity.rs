//\! Multi-platform identity resolve / link / list-links.

use super::*;

/// A resolved Astrid user returned by [`resolve`].
#[derive(Debug)]
pub struct ResolvedUser {
    /// The Astrid-native user ID (UUID).
    pub user_id: String,
    /// Optional display name.
    pub display_name: Option<String>,
}

/// A platform-to-Astrid identity link.
#[derive(Debug)]
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
/// `Ok(None)` if not found. Requires `identity = ["resolve"]` or higher.
pub fn resolve(
    platform: &str,
    platform_user_id: &str,
) -> Result<Option<ResolvedUser>, SysError> {
    let request = wit_types::IdentityResolveRequest {
        platform: platform.to_string(),
        platform_user_id: platform_user_id.to_string(),
    };
    let resp = wit_identity::identity_resolve(&request).map_err(SysError::HostError)?;
    if resp.found {
        let user_id = resp.user_id.ok_or_else(|| {
            SysError::ApiError("host returned found=true but user_id was missing".into())
        })?;
        Ok(Some(ResolvedUser {
            user_id,
            display_name: resp.display_name,
        }))
    } else if let Some(err) = resp.error {
        Err(SysError::ApiError(err))
    } else {
        Ok(None)
    }
}

/// Link a platform identity to an Astrid user.
///
/// - `method` describes how the link was established (e.g. "chat_command", "system").
///
/// Requires `identity = ["link"]` or higher.
pub fn link(
    platform: &str,
    platform_user_id: &str,
    astrid_user_id: &str,
    method: &str,
) -> Result<(), SysError> {
    let request = wit_types::IdentityLinkRequest {
        platform: platform.to_string(),
        platform_user_id: platform_user_id.to_string(),
        astrid_user_id: astrid_user_id.to_string(),
        method: method.to_string(),
    };
    let resp = wit_identity::identity_link(&request).map_err(SysError::HostError)?;
    if !resp.ok {
        return Err(SysError::ApiError(
            resp.error.unwrap_or_else(|| "identity link failed".into()),
        ));
    }
    Ok(())
}

/// Unlink a platform identity from its Astrid user.
///
/// Returns `true` if a link was removed, `false` if none existed.
/// Requires `identity = ["link"]` or higher.
pub fn unlink(platform: &str, platform_user_id: &str) -> Result<bool, SysError> {
    let request = wit_types::IdentityUnlinkRequest {
        platform: platform.to_string(),
        platform_user_id: platform_user_id.to_string(),
    };
    let resp = wit_identity::identity_unlink(&request).map_err(SysError::HostError)?;
    if !resp.ok {
        return Err(SysError::ApiError(
            resp.error
                .unwrap_or_else(|| "identity unlink failed".into()),
        ));
    }
    Ok(resp.removed.unwrap_or(false))
}

/// Create a new Astrid user.
///
/// Returns the UUID of the newly created user.
/// Requires `identity = ["admin"]`.
pub fn create_user(display_name: Option<&str>) -> Result<String, SysError> {
    let request = wit_types::IdentityCreateUserRequest {
        display_name: display_name.map(|s| s.to_string()),
    };
    let resp = wit_identity::identity_create_user(&request).map_err(SysError::HostError)?;
    if !resp.ok {
        return Err(SysError::ApiError(
            resp.error
                .unwrap_or_else(|| "identity create_user failed".into()),
        ));
    }
    resp.user_id
        .ok_or_else(|| SysError::ApiError("missing user_id in response".into()))
}

/// List all platform links for an Astrid user.
///
/// Returns all linked platform identities for the given user UUID.
/// Requires `identity = ["link"]` or higher.
pub fn list_links(astrid_user_id: &str) -> Result<Vec<Link>, SysError> {
    let request = wit_types::IdentityListLinksRequest {
        astrid_user_id: astrid_user_id.to_string(),
    };
    let resp = wit_identity::identity_list_links(&request).map_err(SysError::HostError)?;
    if !resp.ok {
        return Err(SysError::ApiError(
            resp.error
                .unwrap_or_else(|| "identity list_links failed".into()),
        ));
    }
    // Parse links from the links_json field.
    if let Some(json_str) = &resp.links_json {
        #[derive(Deserialize)]
        struct LinkInfo {
            platform: String,
            platform_user_id: String,
            astrid_user_id: String,
            linked_at: String,
            method: String,
        }
        let links: Vec<LinkInfo> = serde_json::from_str(json_str)?;
        Ok(links
            .into_iter()
            .map(|l| Link {
                platform: l.platform,
                platform_user_id: l.platform_user_id,
                astrid_user_id: l.astrid_user_id,
                linked_at: l.linked_at,
                method: l.method,
            })
            .collect())
    } else {
        Ok(Vec::new())
    }
}

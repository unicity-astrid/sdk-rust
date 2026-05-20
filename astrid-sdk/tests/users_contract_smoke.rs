//! Compile-time smoke test: the bundled WIT contracts must expose the
//! users records to downstream consumers. If `astrid:users@1.0.0`
//! drifts out of `astrid-sdk/wit/astrid-contracts.wit`, this fails to
//! compile and CI catches the regression before any capsule does.

#![cfg(feature = "derive")]

use astrid_sdk::contracts::users::{
    AstridUser, ContextClearRequest, ContextClearResponse, ContextGetRequest, ContextGetResponse,
    ContextIdentity, ContextListForUserRequest, ContextListForUserResponse,
    ContextListInContextRequest, ContextListInContextResponse, ContextMember, ContextSetRequest,
    ContextSetResponse, CreateRequest, CreateResponse, DeleteRequest, DeleteResponse, FrontendLink,
    GetRequest, GetResponse, LinkRequest, LinkResponse, LinksRequest, LinksResponse, ListRequest,
    ListResponse, ResolveRequest, ResolveResponse, SetDisplayNameRequest, SetDisplayNameResponse,
    SetPublicKeyRequest, SetPublicKeyResponse, Source, UnlinkRequest, UnlinkResponse,
};

fn assert_send<T: Send>() {}

#[test]
fn users_records_are_send() {
    // Envelope + domain records.
    assert_send::<Source>();
    assert_send::<AstridUser>();
    assert_send::<FrontendLink>();
    assert_send::<ContextIdentity>();
    assert_send::<ContextMember>();

    // Resolve / link / unlink / create.
    assert_send::<ResolveRequest>();
    assert_send::<ResolveResponse>();
    assert_send::<LinkRequest>();
    assert_send::<LinkResponse>();
    assert_send::<UnlinkRequest>();
    assert_send::<UnlinkResponse>();
    assert_send::<CreateRequest>();
    assert_send::<CreateResponse>();

    // AstridUser mutation.
    assert_send::<SetDisplayNameRequest>();
    assert_send::<SetDisplayNameResponse>();
    assert_send::<SetPublicKeyRequest>();
    assert_send::<SetPublicKeyResponse>();

    // Lookup / admin.
    assert_send::<LinksRequest>();
    assert_send::<LinksResponse>();
    assert_send::<GetRequest>();
    assert_send::<GetResponse>();
    assert_send::<DeleteRequest>();
    assert_send::<DeleteResponse>();
    assert_send::<ListRequest>();
    assert_send::<ListResponse>();

    // Context overlay layer.
    assert_send::<ContextSetRequest>();
    assert_send::<ContextSetResponse>();
    assert_send::<ContextClearRequest>();
    assert_send::<ContextClearResponse>();
    assert_send::<ContextGetRequest>();
    assert_send::<ContextGetResponse>();
    assert_send::<ContextListForUserRequest>();
    assert_send::<ContextListForUserResponse>();
    assert_send::<ContextListInContextRequest>();
    assert_send::<ContextListInContextResponse>();
}

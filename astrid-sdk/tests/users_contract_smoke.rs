//! Compile-time smoke test: the bundled WIT contracts must expose the
//! users records to downstream consumers. If `astrid:users@1.0.0`
//! drifts out of `astrid-sdk/wit/astrid-contracts.wit`, this fails to
//! compile and CI catches the regression before any capsule does.

#![cfg(feature = "derive")]

use astrid_sdk::contracts::users::{
    AstridUser, CreateRequest, CreateResponse, DeleteRequest, DeleteResponse, FrontendLink,
    GetRequest, GetResponse, LinkRequest, LinkResponse, LinksRequest, LinksResponse, ListRequest,
    ListResponse, ResolveRequest, ResolveResponse, Source, UnlinkRequest, UnlinkResponse,
};

fn assert_send<T: Send>() {}

#[test]
fn users_records_are_send() {
    assert_send::<Source>();
    assert_send::<AstridUser>();
    assert_send::<FrontendLink>();

    assert_send::<ResolveRequest>();
    assert_send::<ResolveResponse>();
    assert_send::<LinkRequest>();
    assert_send::<LinkResponse>();
    assert_send::<UnlinkRequest>();
    assert_send::<UnlinkResponse>();
    assert_send::<CreateRequest>();
    assert_send::<CreateResponse>();
    assert_send::<LinksRequest>();
    assert_send::<LinksResponse>();
    assert_send::<GetRequest>();
    assert_send::<GetResponse>();
    assert_send::<DeleteRequest>();
    assert_send::<DeleteResponse>();
    assert_send::<ListRequest>();
    assert_send::<ListResponse>();
}

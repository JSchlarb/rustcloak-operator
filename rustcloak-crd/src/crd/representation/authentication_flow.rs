use super::RealmRef;
use crate::keycloak_types::AuthenticationFlowRepresentation;
use crate::{
    KeycloakApiObjectOptions, KeycloakApiStatus, crd::namespace_scope,
    impl_object, schema_patch, traits::impl_endpoint,
};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

namespace_scope! {
    "KeycloakAuthenticationFlow", "kcaf" {
        #[kube(
            doc = "resource to define an Authentication Flow within a [KeycloakRealm](./keycloakrealm.md)",
            group = "rustcloak.k8s.eboland.de",
            version = "v1beta1",
            status = "KeycloakApiStatus",
            category = "keycloak",
            category = "all",
        )]
        /// the KeycloakAuthenticationFlow resource
        pub struct KeycloakAuthenticationFlowSpec {
            #[serde(default, flatten)]
            pub options: Option<KeycloakApiObjectOptions>,
            #[serde(flatten)]
            pub parent_ref: RealmRef,
            #[schemars(schema_with = "schema")]
            pub definition: Option<AuthenticationFlowRepresentation>,
        }
    }
}

impl_object!("authflow" <RealmRef> / |_d| {"authentication/flows"} / "id" for KeycloakAuthenticationFlowSpec => AuthenticationFlowRepresentation);

impl_endpoint!(KeycloakAuthenticationFlow);

schema_patch!(KeycloakAuthenticationFlowSpec);

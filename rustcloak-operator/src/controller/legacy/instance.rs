use super::should_handle_prudent;
use crate::{
    app_id,
    controller::controller_runner::LifecycleController,
    error::Result,
    util::{ApiExt, ApiFactory},
};
use async_trait::async_trait;
use keycloak_crd::ExternalKeycloak as LegacyInstance;
use kube::api::{ObjectMeta, Patch, PatchParams};
use kube::runtime::watcher;
use kube::{
    Api,
    runtime::{Controller, controller::Action},
};
use kube::{Resource, ResourceExt};
use rustcloak_crd::instance::{
    KeycloakInstance, KeycloakInstanceCredentialReference, KeycloakInstanceSpec,
};
use std::sync::Arc;

#[derive(Debug)]
pub struct LegacyInstanceController {
    prudent: bool,
}

impl LegacyInstanceController {
    pub fn new(prudent: bool) -> Self {
        Self { prudent }
    }
}

#[async_trait]
impl LifecycleController for LegacyInstanceController {
    type Resource = LegacyInstance;
    const MODULE_PATH: &'static str = module_path!();

    async fn before_finalizer(
        &self,
        _client: &kube::Client,
        resource: Arc<Self::Resource>,
    ) -> Result<bool> {
        Ok(should_handle_prudent(resource.meta(), self.prudent))
    }

    fn prepare(
        &self,
        controller: Controller<Self::Resource>,
        client: &kube::Client,
    ) -> Controller<Self::Resource> {
        controller.owns(
            Api::<KeycloakInstance>::all(client.clone()),
            watcher::Config::default(),
        )
    }

    async fn apply(
        &self,
        client: &kube::Client,
        resource: Arc<Self::Resource>,
    ) -> Result<Action> {
        let name = resource.name_unchecked();
        let ns = resource.namespace();
        let owner_ref = resource.owner_ref(&()).unwrap();
        let api = ApiExt::<KeycloakInstance>::api(client.clone(), &ns);
        let instance = KeycloakInstance {
            metadata: ObjectMeta {
                name: Some(name.clone()),
                namespace: ns.clone(),
                owner_references: Some(vec![owner_ref]),
                labels: resource.meta().labels.clone(),
                annotations: resource.meta().annotations.clone(),
                ..Default::default()
            },
            spec: KeycloakInstanceSpec {
                base_url: format!(
                    "{}/{}",
                    resource.spec.url.trim_end_matches('/'),
                    resource.spec.context_root.trim_matches('/')
                ),
                realm: Some("master".to_string()),
                credentials: KeycloakInstanceCredentialReference {
                    create: Some(false),
                    secret_name: format!("credential-{}", &name),
                    username_key: Some("ADMIN_USERNAME".to_string()),
                    password_key: Some("ADMIN_PASSWORD".to_string()),
                },
                token: None,
                client: None,
            },
            status: None,
        };
        api.patch(
            &name,
            &PatchParams::apply(app_id!()),
            &Patch::Apply(instance),
        )
        .await?;
        Ok(Action::await_change())
    }

    async fn cleanup(
        &self,
        _client: &kube::Client,
        _resource: Arc<Self::Resource>,
    ) -> Result<Action> {
        Ok(Action::await_change())
    }
}

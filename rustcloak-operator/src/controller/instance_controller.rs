use std::sync::Arc;

use crate::{
    app_id,
    controller::controller_runner::LifecycleController,
    error::{Error, Result},
    util::{K8sKeycloakRefreshManager, RefWatcher, ToPatch},
};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use k8s_openapi::{ByteString, api::core::v1::Secret};
use kube::{
    Api, Resource, ResourceExt,
    api::{ListParams, ObjectMeta, PatchParams, PostParams},
    runtime::{Controller, controller::Action, watcher},
};
use rustcloak_crd::{
    KeycloakApiObject, KeycloakApiStatus, KeycloakInstance,
    traits::SecretKeyNames,
};

use log::warn;
use randstr::randstr;

#[derive(Debug)]
pub struct InstanceController<R>
where
    R: Resource<DynamicType = ()>,
{
    manager: K8sKeycloakRefreshManager,
    secret_refs: Arc<RefWatcher<R, Secret>>,
}

impl Default for InstanceController<KeycloakInstance> {
    fn default() -> Self {
        Self {
            manager: K8sKeycloakRefreshManager::default(),
            secret_refs: Arc::new(RefWatcher::default()),
        }
    }
}

impl InstanceController<KeycloakInstance> {
    async fn create_secret(
        &self,
        client: &kube::Client,
        resource: Arc<KeycloakInstance>,
    ) -> Result<()> {
        let secret_name = &resource.spec.credentials.secret_name;
        let ns = resource.namespace().ok_or(Error::NoNamespace)?;
        let secret_api = Api::<Secret>::namespaced(client.clone(), &ns);
        let [username_key, password_key] =
            resource.spec.credentials.secret_key_names();

        let username = "rustcloak-admin".to_string();
        let password = randstr()
            .must_upper()
            .must_lower()
            .must_digit()
            .must_symbol()
            .len(32)
            .build()
            .generate();
        let data = [
            (username_key.to_string(), ByteString(username.into_bytes())),
            (password_key.to_string(), ByteString(password.into_bytes())),
        ]
        .into();
        let owner_ref = resource.owner_ref(&()).unwrap();

        let secret = Secret {
            data: Some(data),
            metadata: ObjectMeta {
                name: Some(secret_name.to_string()),
                namespace: Some(ns),
                owner_references: Some(vec![owner_ref]),
                ..Default::default()
            },
            type_: Some("Opaque".to_string()),
            ..Default::default()
        };

        secret_api.create(&PostParams::default(), &secret).await?;
        Ok(())
    }
}

#[async_trait]
impl LifecycleController for InstanceController<KeycloakInstance> {
    type Resource = KeycloakInstance;
    const MODULE_PATH: &'static str = module_path!();

    fn prepare(
        &self,
        controller: Controller<Self::Resource>,
        client: &kube::Client,
    ) -> Controller<Self::Resource> {
        let secret_refs = self.secret_refs.clone();
        let secret_api = Api::<Secret>::all(client.clone());
        controller
            .owns(secret_api.clone(), watcher::Config::default())
            .watches(secret_api, watcher::Config::default(), move |secret| {
                secret_refs.watch(&secret)
            })
    }

    async fn before_finalizer(
        &self,
        client: &kube::Client,
        resource: Arc<Self::Resource>,
    ) -> Result<bool> {
        match self
            .manager
            .schedule_refresh(&resource, client.clone())
            .await
        {
            Err(Error::NoCredentialSecret(x, y)) => {
                if resource.spec.credentials.create.unwrap_or(false) {
                    self.create_secret(client, resource.clone()).await?;
                } else {
                    Err(Error::NoCredentialSecret(x, y))?;
                }
            }
            x => x?,
        };

        Ok(true)
    }

    async fn apply(
        &self,
        client: &kube::Client,
        resource: Arc<Self::Resource>,
    ) -> Result<Action> {
        let ns = resource.namespace().ok_or(Error::NoNamespace)?;
        let api = Api::<Self::Resource>::namespaced(client.clone(), &ns);

        self.secret_refs
            .add(&resource, [resource.spec.credential_secret_name()]);

        api.patch_status(
            &resource.name_unchecked(),
            &PatchParams::apply(app_id!()),
            &KeycloakApiStatus::ok("Authenticated").to_patch(),
        )
        .await?;

        Ok(Action::await_change())
    }

    async fn cleanup(
        &self,
        client: &kube::Client,
        resource: Arc<Self::Resource>,
    ) -> Result<Action> {
        let grace_period = Duration::minutes(3);
        let deletion_time =
            resource.meta().deletion_timestamp.as_ref().unwrap().0;

        let selector =
            format!("{}={}", app_id!("instanceRef"), resource.name_unchecked());
        let ns = resource.namespace().ok_or(Error::NoNamespace)?;
        let api = Api::<KeycloakApiObject>::namespaced(client.clone(), &ns);
        let list = api
            .list_metadata(&ListParams::default().labels(&selector))
            .await?;
        if !list.items.is_empty() {
            let items = list.items.iter().map(|item| item.name_any()).collect();
            if Utc::now() < deletion_time + grace_period {
                return Err(Error::ResourceInUseForDeletion(items));
            } else {
                warn!(
                    "Deleting KeycloakApi objects that were not cleaned up, grace period expired. Dangling objects: {:?}",
                    items
                );
            }
        }
        self.manager.cancel_refresh(&resource).await?;
        self.secret_refs.remove(&resource);
        Ok(Action::await_change())
    }
}

use std::{ops::Deref, sync::Arc};

use crate::{
    api::KeycloakClient,
    crd::KeycloakInstance,
    error::{Error, Result},
    util::K8sKeycloakBuilder,
};
use async_stream::stream;
use async_trait::async_trait;
use k8s_openapi::DeepMerge;
use kube::{
    runtime::{controller::Action, Controller},
    Api, ResourceExt,
};
use log::trace;
use reqwest::{Method, Response, StatusCode};
use serde_json::Value;
use tokio::sync::Notify;

use super::controller_runner::LifecycleController;
use crate::crd::KeycloakApiObject;

#[derive(Debug)]
pub struct KeycloakApiObjectController {
    reconcile_notify: Arc<Notify>,
}

impl Default for KeycloakApiObjectController {
    fn default() -> Self {
        let reconcile_notify = Arc::new(Notify::new());
        Self { reconcile_notify }
    }
}

impl KeycloakApiObjectController {
    async fn keycloak(
        client: &kube::Client,
        resource: &KeycloakApiObject,
    ) -> Result<KeycloakClient> {
        let ns = resource.namespace().ok_or(Error::NoNamespace)?;
        let instance_api =
            Api::<KeycloakInstance>::namespaced(client.clone(), &ns);

        let instance_ref = &resource.spec.endpoint.instance_ref;
        let instance = instance_api.get(instance_ref).await?;

        K8sKeycloakBuilder::new(&instance, client)
            .with_token()
            .await
    }

    async fn request(
        &self,
        client: &KeycloakClient,
        method: Method,
        path: &str,
        payload: &Value,
    ) -> Result<Response> {
        let request = client.request(method, path);
        let request = if payload == &Value::Null {
            request
        } else {
            request.json(payload)
        };
        trace!("Request: {:?}", request);
        Ok(request.send().await?.error_for_status()?)
    }
}

#[async_trait]
impl LifecycleController for KeycloakApiObjectController {
    type Resource = KeycloakApiObject;

    fn prepare(
        &self,
        controller: Controller<Self::Resource>,
        _client: &kube::Client,
    ) -> Controller<Self::Resource> {
        let notify = self.reconcile_notify.clone();
        controller.reconcile_all_on(stream! {
            loop {
                notify.notified().await;
                yield;
            }
        })
    }

    async fn apply(
        &self,
        client: &kube::Client,
        resource: Arc<Self::Resource>,
    ) -> Result<Action> {
        let path = &resource.spec.endpoint.path;
        let keycloak = Self::keycloak(client, &resource).await?;
        let mut payload = resource.resolve(client).await?;
        let immutable_payload = resource.spec.immutable_payload.deref();
        payload.merge_from(immutable_payload.clone());
        // First try to PUT, if we get a 404, try to POST
        match self.request(&keycloak, Method::PUT, path, &payload).await {
            Err(Error::ReqwestError(e)) => {
                if e.status() == Some(StatusCode::NOT_FOUND) {
                    let path = path.rsplit_once('/').unwrap().0;
                    self.request(&keycloak, Method::POST, path, &payload)
                        .await?;
                } else {
                    Err(e)?;
                }
            }
            r => {
                r?;
            }
        }
        Ok(Action::await_change())
    }

    async fn cleanup(
        &self,
        client: &kube::Client,
        resource: Arc<Self::Resource>,
    ) -> Result<Action> {
        let path = &resource.spec.endpoint.path;
        let keycloak = Self::keycloak(client, &resource).await?;
        // TODO: handle errors
        let _response = self
            .request(&keycloak, Method::DELETE, path, &Value::Null)
            .await?;

        self.reconcile_notify.notify_one();

        Ok(Action::await_change())
    }
}

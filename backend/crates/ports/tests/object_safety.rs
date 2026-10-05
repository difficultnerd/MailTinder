//! Compile-time checks that every port trait is object safe. Each function
//! takes `Arc<dyn Trait>` and does nothing; the build fails if a trait stops
//! being object safe.

use std::sync::Arc;

use ports::{
    AppFolderStore, Classifier, Clock, HttpEgress, IdentityProvider, InviteMailer, JobScheduler,
    KeyService, MailProvider, Rng, Secrets, SystemKeyService,
};

#[allow(dead_code)]
fn clock(_: Arc<dyn Clock>) {}

#[allow(dead_code)]
fn rng(_: Arc<dyn Rng>) {}

#[allow(dead_code)]
fn mail(_: Arc<dyn MailProvider>) {}

#[allow(dead_code)]
fn app_folder(_: Arc<dyn AppFolderStore>) {}

#[allow(dead_code)]
fn keys(_: Arc<dyn KeyService>) {}

#[allow(dead_code)]
fn system_keys(_: Arc<dyn SystemKeyService>) {}

#[allow(dead_code)]
fn scheduler(_: Arc<dyn JobScheduler>) {}

#[allow(dead_code)]
fn egress(_: Arc<dyn HttpEgress>) {}

#[allow(dead_code)]
fn identity(_: Arc<dyn IdentityProvider>) {}

#[allow(dead_code)]
fn classifier(_: Arc<dyn Classifier>) {}

#[allow(dead_code)]
fn secrets(_: Arc<dyn Secrets>) {}

#[allow(dead_code)]
fn invite_mailer(_: Arc<dyn InviteMailer>) {}

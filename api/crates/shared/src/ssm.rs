//! `SecureString` parameters read from SSM once per execution environment.

use crate::Error;
use tokio::sync::OnceCell;

/// An SSM parameter fetched on first use and cached afterward.
pub struct Parameter {
    client: aws_sdk_ssm::Client,
    name: String,
    value: OnceCell<String>,
}

impl Parameter {
    #[must_use]
    pub fn new(client: aws_sdk_ssm::Client, name: String) -> Self {
        Self {
            client,
            name,
            value: OnceCell::new(),
        }
    }

    /// The decrypted value.
    ///
    /// # Errors
    ///
    /// If SSM can't be reached or the parameter has no value.
    pub async fn value(&self) -> Result<&String, Error> {
        self.value
            .get_or_try_init(|| async {
                self.client
                    .get_parameter()
                    .name(&self.name)
                    .with_decryption(true)
                    .send()
                    .await?
                    .parameter
                    .and_then(|parameter| parameter.value)
                    .ok_or_else(|| format!("SSM parameter {} has no value", self.name).into())
            })
            .await
    }
}

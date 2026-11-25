//! Canned SteamGridDB answers, so tests run the production fetch without
//! reaching the network.

use std::sync::Arc;

use super::{Fetcher, fetch_default_over};

/// SteamGridDB's answers for a game with an icon, and nothing else, by
/// address.
type Hook = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub(crate) struct Canned(
    pub(crate) Arc<std::collections::HashMap<String, (u16, Vec<u8>)>>,
    /// Runs on each request whose address holds its text.
    pub(crate) Option<(&'static str, Hook)>,
);

impl wineo_core::download::Transport for Canned {
    fn get(&self, url: &str) -> anyhow::Result<Box<dyn std::io::Read>> {
        anyhow::bail!("unexpected request to {url}")
    }

    fn request(
        &self,
        url: &str,
        _: Option<&str>,
    ) -> anyhow::Result<wineo_core::download::Response> {
        if let Some((at, hook)) = &self.1
            && url.contains(at)
        {
            hook();
        }
        let Some((status, body)) = self.0.get(url) else {
            anyhow::bail!("connection to {url} failed");
        };
        Ok(wineo_core::download::Response {
            status: *status,
            body: Box::new(std::io::Cursor::new(body.clone())),
        })
    }
}

/// A one pixel PNG, which the desktop entry can decode as its icon.
pub(crate) const ONE_PIXEL_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0\x1f\x15\xc4\x89\0\0\0\rIDATx\x9c\x63\xf8\xcf\xc0\xf0\x1f\0\x05\0\x01\xff\x89\x99\x3d\x1d\0\0\0\0IEND\xaeB`\x82";

pub(crate) fn canned_hades() -> Canned {
    use wineo_core::steamgriddb::{API, Api, Filters};
    let mut answers = std::collections::HashMap::new();
    let json = |body: &str| (200, body.as_bytes().to_vec());
    answers.insert(
        format!("{API}/search/autocomplete/Hades"),
        json(r#"{"success":true,"data":[{"id":7,"name":"Hades"}]}"#),
    );
    for kind in wineo_core::art::Kind::ALL {
        let page = |number| Api::images_url(7, kind, number, Filters::Ranked);
        let first = if kind == wineo_core::art::Kind::Icon {
            r#"{"success":true,"data":[{"id":1,"url":"https://cdn.example/1"}]}"#
        } else {
            r#"{"success":true,"data":[]}"#
        };
        answers.insert(page(0), json(first));
        answers.insert(page(1), json(r#"{"success":true,"data":[]}"#));
    }
    answers.insert(
        "https://cdn.example/1".to_owned(),
        (200, ONE_PIXEL_PNG.to_vec()),
    );
    Canned(Arc::new(answers), None)
}

impl Canned {
    /// Runs `hook` on each request whose address holds `at`.
    pub(crate) fn on(mut self, at: &'static str, hook: impl Fn() + Send + Sync + 'static) -> Self {
        self.1 = Some((at, Arc::new(hook)));
        self
    }

    /// The production fetch, over these answers.
    pub(crate) fn fetcher(self) -> Fetcher {
        Arc::new(move |paths, name, title, identity, token| {
            let answers = self.clone();
            fetch_default_over(paths, name, title, identity, token, &move || {
                Ok((Box::new(answers.clone()), "key".to_owned()))
            })
        })
    }
}

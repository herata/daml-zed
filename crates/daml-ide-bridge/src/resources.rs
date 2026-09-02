//! The set of script results the bridge is currently showing.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;

use crate::ids::resource_id;

#[derive(Debug, Clone)]
pub struct Resource {
    pub id: String,
    pub title: String,
    /// `None` until the server has rendered it at least once.
    pub html: Option<String>,
    pub running: bool,
}

#[derive(Default)]
pub struct Registry {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    by_id: HashMap<String, Resource>,
    id_by_uri: HashMap<String, String>,
    subscribers: HashMap<String, Vec<Sender<()>>>,
}

impl Registry {
    pub fn register(&self, title: &str, uri: &str) -> String {
        let id = resource_id(uri);
        let mut inner = self.inner.lock().unwrap();
        inner.id_by_uri.insert(uri.to_string(), id.clone());
        inner.by_id.entry(id.clone()).or_insert_with(|| Resource {
            id: id.clone(),
            title: title.to_string(),
            html: None,
            running: true,
        });
        id
    }

    pub fn is_known(&self, uri: &str) -> bool {
        self.inner.lock().unwrap().id_by_uri.contains_key(uri)
    }

    pub fn update(&self, uri: &str, html: &str) {
        let mut inner = self.inner.lock().unwrap();
        let Some(id) = inner.id_by_uri.get(uri).cloned() else {
            return;
        };
        if let Some(resource) = inner.by_id.get_mut(&id) {
            resource.html = Some(html.to_string());
            resource.running = false;
        }
        notify(&mut inner, &id);
    }

    pub fn set_running(&self, uri: &str) {
        let mut inner = self.inner.lock().unwrap();
        let Some(id) = inner.id_by_uri.get(uri).cloned() else {
            return;
        };
        if let Some(resource) = inner.by_id.get_mut(&id) {
            resource.running = true;
        }
        notify(&mut inner, &id);
    }

    pub fn get(&self, id: &str) -> Option<Resource> {
        self.inner.lock().unwrap().by_id.get(id).cloned()
    }

    pub fn list(&self) -> Vec<Resource> {
        let mut all: Vec<_> = self.inner.lock().unwrap().by_id.values().cloned().collect();
        all.sort_by(|a, b| a.title.cmp(&b.title));
        all
    }

    pub fn subscribe(&self, id: &str) -> Option<Receiver<()>> {
        let mut inner = self.inner.lock().unwrap();
        inner.by_id.get(id)?;
        let (tx, rx) = channel();
        inner
            .subscribers
            .entry(id.to_string())
            .or_default()
            .push(tx);
        Some(rx)
    }
}

/// A closed browser tab drops its receiver, which is how it unsubscribes.
fn notify(inner: &mut Inner, id: &str) {
    if let Some(subscribers) = inner.subscribers.get_mut(id) {
        subscribers.retain(|tx| tx.send(()).is_ok());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_and_updates() {
        let reg = Registry::default();
        let id = reg.register("Script: setup", "daml://x");
        assert!(reg.get(&id).unwrap().html.is_none());

        reg.update("daml://x", "<html>1</html>");
        assert_eq!(
            reg.get(&id).unwrap().html.as_deref(),
            Some("<html>1</html>")
        );

        reg.update("daml://x", "<html>2</html>");
        assert_eq!(
            reg.get(&id).unwrap().html.as_deref(),
            Some("<html>2</html>")
        );
    }

    #[test]
    fn the_same_uri_keeps_the_same_id() {
        let reg = Registry::default();
        assert_eq!(reg.register("a", "daml://x"), reg.register("a", "daml://x"));
        assert_eq!(reg.list().len(), 1);
    }

    #[test]
    fn an_update_for_an_unknown_uri_is_ignored() {
        let reg = Registry::default();
        reg.update("daml://never-seen", "<html/>");
        assert!(reg.list().is_empty());
    }

    #[test]
    fn a_render_clears_the_running_flag_and_progress_sets_it() {
        let reg = Registry::default();
        let id = reg.register("a", "daml://x");
        assert!(reg.get(&id).unwrap().running);
        reg.update("daml://x", "<html/>");
        assert!(!reg.get(&id).unwrap().running);
        reg.set_running("daml://x");
        assert!(reg.get(&id).unwrap().running);
    }

    #[test]
    fn subscribers_are_woken_on_update() {
        let reg = Registry::default();
        let id = reg.register("a", "daml://x");
        let rx = reg.subscribe(&id).unwrap();
        reg.update("daml://x", "<html/>");
        assert!(rx.recv_timeout(std::time::Duration::from_secs(1)).is_ok());
    }

    #[test]
    fn subscribing_to_an_unknown_id_fails() {
        assert!(Registry::default().subscribe("nope").is_none());
    }
}

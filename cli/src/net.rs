//! HTTP with a strict redirect rule.
//!
//! A provider or an API may answer with a redirect. Following it can send an `Authorization` header to another host,
//! which is a key leak the privacy filter cannot see. Every request of Penelope therefore uses an agent with redirects
//! switched off; a 3xx is returned as an error or as a status, never followed.

/// An agent without redirects and with a timeout in seconds.
pub fn agent(seconds: u64) -> ureq::Agent {
    ureq::AgentBuilder::new().redirects(0).timeout(std::time::Duration::from_secs(seconds)).build()
}

#[cfg(test)]
mod tests {
    use super::agent;
    #[test]
    fn a_redirect_is_not_followed() {
        // The fake server answers 302 once. The agent must return that status, not ask the Location target.
        let (address, seen) = crate::llm::fake::serve(vec![(302, "moved".to_string())]);
        let result = agent(5).get(&format!("{}/first", address)).call();
        match result {
            Ok(r) => assert_eq!(r.status(), 302),
            Err(ureq::Error::Status(302, _)) => {}
            Err(e) => panic!("expected status 302, got {}", e),
        }
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
}

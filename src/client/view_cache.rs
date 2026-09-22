//! In-memory session cache for instant screen switches (stale-while-revalidate).

use std::collections::{HashMap, VecDeque};

use crate::shared::{DmConversation, DmMessage, Notification, Post, UserProfile};

use crate::client::phase_c::FeedTab;

const PROFILE_CAP: usize = 32;
const THREAD_CAP: usize = 32;
const DM_THREAD_CAP: usize = 32;

#[derive(Debug, Clone)]
pub struct CachedProfile {
    pub profile: UserProfile,
    pub posts: Vec<Post>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Clone)]
pub struct CachedThread {
    pub root_id: String,
    pub posts: Vec<Post>,
}

#[derive(Debug, Clone)]
pub struct CachedDmThread {
    pub conversation_id: String,
    pub peer_handle: String,
    pub messages: Vec<DmMessage>,
}

#[derive(Debug, Clone)]
pub struct CachedFeed {
    pub posts: Vec<Post>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug)]
struct LruMap<V> {
    map: HashMap<String, V>,
    order: VecDeque<String>,
    cap: usize,
}

impl<V> LruMap<V> {
    fn new(cap: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            cap,
        }
    }

    fn get(&self, key: &str) -> Option<&V> {
        self.map.get(key)
    }

    fn insert(&mut self, key: String, value: V) -> Option<String> {
        if let Some(pos) = self.order.iter().position(|k| k == &key) {
            self.order.remove(pos);
        }
        self.order.push_back(key.clone());
        self.map.insert(key, value);
        let mut evicted = None;
        while self.map.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                if self.map.remove(&old).is_some() {
                    evicted = Some(old);
                }
            } else {
                break;
            }
        }
        evicted
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.len()
    }
}

#[derive(Debug)]
pub struct ViewCache {
    profiles: LruMap<CachedProfile>,
    threads: LruMap<CachedThread>,
    post_to_root: HashMap<String, String>,
    notifications: Option<Vec<Notification>>,
    dm_list: Option<Vec<DmConversation>>,
    dm_threads: LruMap<CachedDmThread>,
    for_you: Option<CachedFeed>,
    following: Option<CachedFeed>,
}

impl Default for ViewCache {
    fn default() -> Self {
        Self {
            profiles: LruMap::new(PROFILE_CAP),
            threads: LruMap::new(THREAD_CAP),
            post_to_root: HashMap::new(),
            notifications: None,
            dm_list: None,
            dm_threads: LruMap::new(DM_THREAD_CAP),
            for_you: None,
            following: None,
        }
    }
}

impl ViewCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    // ── Feed tabs ───────────────────────────────────────────────────────────

    pub fn stash_feed(&mut self, tab: FeedTab, posts: Vec<Post>) {
        let snap = CachedFeed {
            next_cursor: posts.last().map(|p| p.id.clone()),
            has_more: true,
            posts,
        };
        match tab {
            FeedTab::All => self.for_you = Some(snap),
            FeedTab::Following => self.following = Some(snap),
        }
    }

    pub fn stash_feed_page(
        &mut self,
        tab: FeedTab,
        posts: Vec<Post>,
        next_cursor: Option<String>,
        has_more: bool,
    ) {
        let snap = CachedFeed {
            posts,
            next_cursor,
            has_more,
        };
        match tab {
            FeedTab::All => self.for_you = Some(snap),
            FeedTab::Following => self.following = Some(snap),
        }
    }

    pub fn feed_page(&self, tab: FeedTab) -> Option<&CachedFeed> {
        match tab {
            FeedTab::All => self.for_you.as_ref(),
            FeedTab::Following => self.following.as_ref(),
        }
    }

    pub fn set_feed(&mut self, tab: FeedTab, posts: Vec<Post>) {
        self.stash_feed(tab, posts);
    }

    // ── Profiles ────────────────────────────────────────────────────────────

    pub fn profile(&self, handle: &str) -> Option<&CachedProfile> {
        self.profiles.get(&handle.to_ascii_lowercase())
    }

    pub fn put_profile(&mut self, profile: UserProfile, posts: Vec<Post>) {
        self.put_profile_page(profile, posts, None, false);
    }

    pub fn put_profile_page(
        &mut self,
        profile: UserProfile,
        posts: Vec<Post>,
        next_cursor: Option<String>,
        has_more: bool,
    ) {
        let key = profile.handle.to_ascii_lowercase();
        self.profiles.insert(
            key,
            CachedProfile {
                profile,
                posts,
                next_cursor,
                has_more,
            },
        );
    }

    // ── Threads ─────────────────────────────────────────────────────────────

    /// Resolve a thread from any post id in it (or root).
    pub fn thread_for_post(&self, post_id: &str) -> Option<&CachedThread> {
        if let Some(root) = self.post_to_root.get(post_id) {
            return self.threads.get(root);
        }
        self.threads.get(post_id)
    }

    pub fn put_thread(&mut self, root_id: String, posts: Vec<Post>) {
        for p in &posts {
            self.post_to_root.insert(p.id.clone(), root_id.clone());
        }
        if let Some(evicted) = self.threads.insert(
            root_id.clone(),
            CachedThread {
                root_id: root_id.clone(),
                posts,
            },
        ) {
            self.post_to_root.retain(|_, root| root != &evicted);
        }
    }

    // ── Notifications ───────────────────────────────────────────────────────

    pub fn notifications(&self) -> Option<&[Notification]> {
        self.notifications.as_deref()
    }

    pub fn put_notifications(&mut self, items: Vec<Notification>) {
        self.notifications = Some(items);
    }

    // ── DMs ─────────────────────────────────────────────────────────────────

    pub fn dm_list(&self) -> Option<&[DmConversation]> {
        self.dm_list.as_deref()
    }

    pub fn put_dm_list(&mut self, items: Vec<DmConversation>) {
        self.dm_list = Some(items);
    }

    pub fn dm_thread(&self, id: &str) -> Option<&CachedDmThread> {
        self.dm_threads.get(id)
    }

    pub fn dm_thread_by_peer(&self, handle: &str) -> Option<&CachedDmThread> {
        let h = handle.to_ascii_lowercase();
        self.dm_threads.map.values().find(|t| t.peer_handle == h)
    }

    pub fn put_dm_thread(
        &mut self,
        conversation_id: String,
        peer_handle: String,
        messages: Vec<DmMessage>,
    ) {
        let peer = peer_handle.to_ascii_lowercase();
        self.dm_threads.insert(
            conversation_id.clone(),
            CachedDmThread {
                conversation_id,
                peer_handle: peer,
                messages,
            },
        );
    }

    /// Update DM list preview when a live message arrives.
    pub fn touch_dm_preview(&mut self, msg: &DmMessage) {
        if let Some(list) = self.dm_list.as_mut() {
            if let Some(c) = list.iter_mut().find(|c| c.id == msg.conversation_id) {
                c.last_message = Some(msg.content.clone());
                c.last_at = Some(msg.created_at);
                if let Some(pos) = list.iter().position(|c| c.id == msg.conversation_id) {
                    let c = list.remove(pos);
                    list.insert(0, c);
                }
            }
        }
        if let Some(th) = self.dm_threads.map.get_mut(&msg.conversation_id) {
            if !th.messages.iter().any(|m| m.id == msg.id) {
                th.messages.push(msg.clone());
            }
        }
    }

    pub fn bump_dm_unread(&mut self, conversation_id: &str, from_peer: bool) {
        if !from_peer {
            return;
        }
        if let Some(list) = self.dm_list.as_mut() {
            if let Some(c) = list.iter_mut().find(|c| c.id == conversation_id) {
                c.unread = c.unread.saturating_add(1);
            }
        }
    }

    pub fn clear_dm_unread(&mut self, conversation_id: &str) {
        if let Some(list) = self.dm_list.as_mut() {
            if let Some(c) = list.iter_mut().find(|c| c.id == conversation_id) {
                c.unread = 0;
            }
        }
    }

    pub fn unread_dms(&self) -> i64 {
        self.dm_list
            .as_ref()
            .map(|list| list.iter().map(|c| c.unread).sum())
            .unwrap_or(0)
    }
}

pub fn merge_posts_append(existing: &[Post], incoming: Vec<Post>) -> Vec<Post> {
    let mut out = existing.to_vec();
    for p in incoming {
        if !out.iter().any(|e| e.id == p.id) {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::UserProfile;
    use chrono::Utc;

    fn profile(handle: &str) -> UserProfile {
        UserProfile {
            id: handle.into(),
            handle: handle.into(),
            bio: None,
            created_at: Utc::now(),
            followers: 0,
            following: 0,
            posts: 0,
            followed_by_me: false,
            follows_me: false,
            blocked_by_me: false,
            is_me: false,
        }
    }

    #[test]
    fn evicts_oldest_profile() {
        let mut cache = ViewCache::new();
        for i in 0..(PROFILE_CAP + 3) {
            cache.put_profile(profile(&format!("u{i}")), Vec::new());
        }
        assert_eq!(cache.profiles.len(), PROFILE_CAP);
        assert!(cache.profile("u0").is_none());
        assert!(cache.profile(&format!("u{}", PROFILE_CAP + 2)).is_some());
    }

    #[test]
    fn merge_skips_duplicates() {
        let a = Post {
            id: "a".into(),
            author_id: "u".into(),
            author_handle: "alice".into(),
            parent_id: None,
            quote_id: None,
            quote: None,
            content: "1".into(),
            created_at: Utc::now(),
            deleted_at: None,
            edited_at: None,
            like_count: 0,
            liked_by_me: false,
            reply_count: 0,
            images: Vec::new(),
        };
        let mut b = a.clone();
        b.id = "b".into();
        let merged = merge_posts_append(&[a.clone()], vec![a.clone(), b.clone()]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1].id, "b");
    }
}

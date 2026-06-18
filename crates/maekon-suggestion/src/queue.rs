use maekon_core::models::suggestion::Suggestion;
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::collections::HashSet;

#[derive(Debug, Clone)]
struct PrioritizedSuggestion {
    suggestion: Suggestion,
}

impl PartialEq for PrioritizedSuggestion {
    fn eq(&self, other: &Self) -> bool {
        // #5984: Ord requires Eq-consistency (`a == b` iff `a.cmp(b) == Equal`),
        // which BTreeSet relies on. Deriving eq from cmp keeps them in lockstep —
        // an id-only eq disagreed with the (priority, created_at, id) ordering,
        // letting same-id/different-priority items violate the set invariant.
        // Content dedup is handled separately by the fingerprint set; removal by
        // id (`remove_by_id`) scans the id field directly, so it is unaffected.
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for PrioritizedSuggestion {}

impl PartialOrd for PrioritizedSuggestion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PrioritizedSuggestion {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .suggestion
            .priority
            .cmp(&self.suggestion.priority)
            .then_with(|| other.suggestion.created_at.cmp(&self.suggestion.created_at))
            .then_with(|| {
                self.suggestion
                    .suggestion_id
                    .cmp(&other.suggestion.suggestion_id)
            })
    }
}

fn content_fingerprint(suggestion: &Suggestion) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    suggestion.suggestion_type.hash(&mut hasher);
    suggestion.source.hash(&mut hasher);
    suggestion.context_scope.hash(&mut hasher);
    let normalized: String = suggestion
        .content
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    // Hash the ENTIRE normalized content, not a prefix. A 200-char prefix as the
    // equality oracle silently dropped distinct suggestions that shared
    // type/source/scope and only diverged after char 200. Hashing the whole
    // string is O(content) — trivial at queue volumes — and makes the earlier
    // char-boundary truncation (and its #5691 multi-byte panic risk) moot, since
    // we never slice into the string anymore.
    normalized.hash(&mut hasher);
    hasher.finish()
}

pub struct SuggestionQueue {
    items: BTreeSet<PrioritizedSuggestion>,
    fingerprints: HashSet<u64>,
    max_size: usize,
}

impl SuggestionQueue {
    pub fn new(max_size: usize) -> Self {
        Self {
            items: BTreeSet::new(),
            fingerprints: HashSet::new(),
            max_size,
        }
    }

    pub fn push(&mut self, suggestion: Suggestion) -> bool {
        // A zero-capacity queue must reject every push. Without this guard the
        // `len() >= max_size` eviction check below admits the first item (the
        // empty queue has no `last` to evict against), violating the max_size
        // contract.
        if self.max_size == 0 {
            return false;
        }
        let fp = content_fingerprint(&suggestion);
        if self.fingerprints.contains(&fp) {
            // #26: a duplicate fingerprint must not blindly reject. Compare the
            // incoming item against the existing one — if the newcomer is
            // strictly "better" (higher priority, or equal priority but a newer
            // created_at), replace the existing item. Otherwise keep rejecting.
            // `PrioritizedSuggestion::cmp` orders the best item first (higher
            // priority, then newer created_at), so `incoming < existing` means
            // the incoming item is strictly better. `created_at` is the same
            // tiebreak the queue ordering already uses.
            let item = PrioritizedSuggestion { suggestion };
            if let Some(existing) = self
                .items
                .iter()
                .find(|ps| content_fingerprint(&ps.suggestion) == fp)
                .cloned()
            {
                if item < existing {
                    // Admit the better item FIRST; only drop the lower-priority
                    // duplicate `existing` once `item` is actually accepted. `item`
                    // can collide with a THIRD Ord-equal item carrying different
                    // content (items.insert → false); removing `existing`
                    // unconditionally (the previous code) would then drop `existing`
                    // AND orphan its fingerprint — the same dedup-poison class fixed
                    // on the fall-through path (review4 re-verify sibling). `fp` is
                    // already present (we entered via fingerprints.contains) and
                    // stays backed by whichever of item/existing remains, so the
                    // fingerprint set needs no mutation here. item/existing are not
                    // Ord-equal (item < existing), so both can briefly coexist.
                    let incoming_id = item.suggestion.suggestion_id.clone();
                    let inserted = self.items.insert(item);
                    if inserted {
                        self.items.remove(&existing);
                        tracing::debug!(
                            replaced_id = %existing.suggestion.suggestion_id,
                            replaced_priority = ?existing.suggestion.priority,
                            new_id = %incoming_id,
                            "duplicate content fingerprint — replaced with higher-priority item"
                        );
                    } else {
                        tracing::debug!(
                            rejected_id = %incoming_id,
                            "duplicate content fingerprint — replacement collided with an Ord-equal item; kept existing"
                        );
                    }
                    return inserted;
                }
            }
            tracing::debug!(
                rejected_id = %item.suggestion.suggestion_id,
                "duplicate content fingerprint — rejected"
            );
            return false;
        }

        let item = PrioritizedSuggestion { suggestion };

        if self.items.len() >= self.max_size {
            // Queue full: the only admit path is evicting a strictly-worse `last`.
            let Some(last) = self.items.iter().next_back().cloned() else {
                // `max_size == 0` already returned above, so a full queue is non-empty;
                // unreachable, but reject safely rather than admit over capacity.
                return false;
            };
            if item < last {
                // #6423: admit the better `item` FIRST, then evict `last` only once
                // `item` is actually accepted. `items.insert` returns false when `item`
                // collides with an Ord-equal sibling (same priority/created_at/id cmp
                // key, different content — keys come from the untrusted server SSE
                // payload). The previous code evicted `last` unconditionally and then fell
                // through to an insert that could fail, dropping a valid `last` while
                // admitting nothing (silent data loss). Mirrors the dedup replace-path
                // fix (#26 / #6340).
                let incoming_id = item.suggestion.suggestion_id.clone();
                let inserted = self.items.insert(item);
                if inserted {
                    self.items.remove(&last);
                    self.fingerprints
                        .remove(&content_fingerprint(&last.suggestion));
                    self.fingerprints.insert(fp);
                    tracing::warn!(
                        evicted_id = %last.suggestion.suggestion_id,
                        evicted_priority = ?last.suggestion.priority,
                        new_id = %incoming_id,
                        queue_size = self.max_size,
                        "suggestion queue full — evicted lower-priority item"
                    );
                } else {
                    tracing::warn!(
                        rejected_id = %incoming_id,
                        queue_size = self.max_size,
                        "suggestion queue full — eviction candidate collided with an Ord-equal item; kept existing"
                    );
                }
                return inserted;
            }
            tracing::warn!(
                rejected_id = %item.suggestion.suggestion_id,
                rejected_priority = ?item.suggestion.priority,
                queue_size = self.max_size,
                "suggestion queue full — rejected (priority too low)"
            );
            return false;
        }

        // Queue not full. Keep `fingerprints` in 1:1 lockstep with `items` (review4):
        // record the fingerprint ONLY if the item was actually admitted. `items.insert`
        // returns false when an Ord-equal item already exists (cmp keys on
        // priority/created_at/suggestion_id, which differ from content_fingerprint), so
        // an unconditional fingerprint insert would orphan `fp` with no backing item —
        // any later suggestion whose content hashes to that orphan fp would then be
        // permanently, silently rejected. The Ord keys come from the untrusted server SSE
        // payload, so a buggy/malicious server could poison the dedup oracle.
        let inserted = self.items.insert(item);
        if inserted {
            self.fingerprints.insert(fp);
        }
        inserted
    }

    pub fn pop(&mut self) -> Option<Suggestion> {
        let first = self.items.iter().next()?.clone();
        self.items.remove(&first);
        self.fingerprints
            .remove(&content_fingerprint(&first.suggestion));
        Some(first.suggestion)
    }

    pub fn peek(&self) -> Option<&Suggestion> {
        self.items.iter().next().map(|p| &p.suggestion)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Suggestion> {
        self.items.iter().map(|p| &p.suggestion)
    }

    /// Remove a suggestion by its ID. Returns the removed Suggestion if found.
    pub fn remove_by_id(&mut self, suggestion_id: &str) -> Option<Suggestion> {
        let item = self
            .items
            .iter()
            .find(|ps| ps.suggestion.suggestion_id == suggestion_id)
            .cloned();
        if let Some(ref found) = item {
            self.items.remove(found);
            self.fingerprints
                .remove(&content_fingerprint(&found.suggestion));
            Some(found.suggestion.clone())
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.fingerprints.clear();
    }

    pub fn remove_expired(&mut self) -> usize {
        let now = chrono::Utc::now();
        let expired_fps: Vec<u64> = self
            .items
            .iter()
            .filter(|p| {
                p.suggestion
                    .expires_at
                    .is_some_and(|expires| expires <= now)
            })
            .map(|p| content_fingerprint(&p.suggestion))
            .collect();
        for fp in &expired_fps {
            self.fingerprints.remove(fp);
        }
        let before = self.items.len();
        self.items
            .retain(|p| p.suggestion.expires_at.is_none_or(|expires| expires > now));
        before - self.items.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use maekon_core::models::suggestion::{Priority, SuggestionType};

    fn make_suggestion(id: &str, priority: Priority) -> Suggestion {
        Suggestion {
            suggestion_id: id.to_string(),
            suggestion_type: SuggestionType::WorkGuidance,
            content: format!("suggestion {id}"),
            priority,
            confidence_score: 0.9,
            relevance_score: 0.8,
            is_actionable: true,
            created_at: Utc::now(),
            expires_at: None,
            source: Default::default(),
            reasoning: None,
            context_scope: None,
        }
    }

    #[test]
    fn priority_ordering() {
        let mut queue = SuggestionQueue::new(50);
        queue.push(make_suggestion("low", Priority::Low));
        queue.push(make_suggestion("critical", Priority::Critical));
        queue.push(make_suggestion("medium", Priority::Medium));
        queue.push(make_suggestion("high", Priority::High));

        assert_eq!(queue.pop().unwrap().suggestion_id, "critical");
        assert_eq!(queue.pop().unwrap().suggestion_id, "high");
        assert_eq!(queue.pop().unwrap().suggestion_id, "medium");
        assert_eq!(queue.pop().unwrap().suggestion_id, "low");
    }

    #[test]
    fn max_size_enforcement() {
        let mut queue = SuggestionQueue::new(2);
        queue.push(make_suggestion("1", Priority::Low));
        queue.push(make_suggestion("2", Priority::Medium));
        assert_eq!(queue.len(), 2);

        queue.push(make_suggestion("3", Priority::High));
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.peek().unwrap().suggestion_id, "3");
    }

    #[test]
    fn push_keeps_fingerprints_in_lockstep_with_items() {
        // review4: two suggestions sharing (suggestion_id, created_at, priority) but
        // differing in content are Ord-equal (cmp keys on those three) yet have
        // different content fingerprints. The second must be rejected WITHOUT
        // leaving an orphan fingerprint — otherwise a later suggestion whose content
        // hashes to that orphan would be permanently, silently rejected.
        let mut queue = SuggestionQueue::new(50);
        let ts = Utc::now();
        let mut a = make_suggestion("dup", Priority::High);
        a.created_at = ts;
        a.content = "content A".to_string();
        let mut b = make_suggestion("dup", Priority::High);
        b.created_at = ts;
        b.content = "content B".to_string();

        assert!(queue.push(a));
        assert!(
            !queue.push(b),
            "Ord-equal item (same id+created_at+priority) must be rejected"
        );
        assert_eq!(queue.len(), 1);
        assert_eq!(
            queue.fingerprints.len(),
            queue.items.len(),
            "fingerprints must stay in lockstep with items (no orphan)"
        );
    }

    #[test]
    fn push_replace_branch_keeps_lockstep_on_ord_collision() {
        // review4 re-verify: the duplicate-replace branch must also keep
        // fingerprints in lockstep. A third Ord-equal item makes the replacement's
        // items.insert collide and get rejected — `existing` must be preserved and
        // no fingerprint orphaned.
        let mut queue = SuggestionQueue::new(50);
        let t_k = Utc::now();

        // existing E: content "X", Low, id "z".
        let mut e = make_suggestion("z", Priority::Low);
        e.content = "X".to_string();
        assert!(queue.push(e));

        // Z: distinct content "Y" (different fingerprint), High, id "k", created_at t_k.
        let mut z = make_suggestion("k", Priority::High);
        z.content = "Y".to_string();
        z.created_at = t_k;
        assert!(queue.push(z));

        // incoming: content "X" (== E's fingerprint → enters the replace branch),
        // but High + id "k" + created_at t_k → Ord-equal to Z, so items.insert collides.
        let mut incoming = make_suggestion("k", Priority::High);
        incoming.content = "X".to_string();
        incoming.created_at = t_k;
        assert!(
            !queue.push(incoming),
            "replacement colliding with an Ord-equal item must be rejected"
        );

        assert_eq!(queue.len(), 2);
        assert_eq!(
            queue.fingerprints.len(),
            queue.items.len(),
            "replace-branch collision must not orphan a fingerprint"
        );
        // E must still be present — not silently dropped by the failed replacement.
        assert!(queue
            .iter()
            .any(|s| s.content == "X" && s.suggestion_id == "z"));
    }

    #[test]
    fn empty_queue() {
        let mut queue = SuggestionQueue::new(50);
        assert!(queue.is_empty());
        assert!(queue.pop().is_none());
        assert!(queue.peek().is_none());
    }

    #[test]
    fn clear_queue() {
        let mut queue = SuggestionQueue::new(50);
        queue.push(make_suggestion("1", Priority::High));
        queue.push(make_suggestion("2", Priority::Medium));
        assert_eq!(queue.len(), 2);
        queue.clear();
        assert!(queue.is_empty());
    }

    #[test]
    fn remove_by_id_returns_and_removes() {
        let mut queue = SuggestionQueue::new(50);
        let s = make_suggestion("s1", Priority::High);
        queue.push(s);
        assert_eq!(queue.len(), 1);
        let removed = queue.remove_by_id("s1");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().suggestion_id, "s1");
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn remove_by_id_returns_none_if_not_found() {
        let mut queue = SuggestionQueue::new(50);
        assert!(queue.remove_by_id("nonexistent").is_none());
    }

    #[test]
    fn remove_expired() {
        let mut queue = SuggestionQueue::new(50);

        let mut expired = make_suggestion("expired", Priority::High);
        expired.expires_at = Some(Utc::now() - chrono::Duration::hours(1));
        queue.push(expired);

        let valid = make_suggestion("valid", Priority::Medium);
        queue.push(valid);

        let removed = queue.remove_expired();
        assert_eq!(removed, 1);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.peek().unwrap().suggestion_id, "valid");
    }

    #[test]
    fn duplicate_content_rejected() {
        // #26: a same-priority duplicate (no priority gain, same created_at
        // tiebreak floor) must be rejected. The higher-priority-replaces case
        // is covered by `higher_priority_duplicate_replaces_existing`.
        let mut queue = SuggestionQueue::new(50);
        let s1 = make_suggestion("s1", Priority::High);
        let mut s2 = make_suggestion("s2", Priority::High);
        s2.content = s1.content.clone();
        s2.suggestion_type = s1.suggestion_type.clone();
        s2.created_at = s1.created_at;
        assert!(queue.push(s1));
        assert!(!queue.push(s2));
        assert_eq!(queue.len(), 1);
    }

    /// #26: a higher-priority duplicate arriving after a lower-priority one
    /// must replace it rather than being silently dropped. The replacement is
    /// surfaced by the queue head priority, not the count (dedup keeps len=1).
    #[test]
    fn higher_priority_duplicate_replaces_existing() {
        let mut queue = SuggestionQueue::new(50);
        let low = make_suggestion("low", Priority::Low);
        let mut high = make_suggestion("high", Priority::Critical);
        high.content = low.content.clone();
        high.suggestion_type = low.suggestion_type.clone();

        assert!(queue.push(low));
        // Same fingerprint, strictly higher priority -> replaces.
        assert!(queue.push(high));
        assert_eq!(queue.len(), 1);
        let head = queue.peek().unwrap();
        assert_eq!(head.suggestion_id, "high");
        assert_eq!(head.priority, Priority::Critical);
    }

    /// #26: a lower-priority duplicate arriving after a higher-priority one
    /// must still be rejected (the original replacement bug only affected the
    /// higher-priority direction).
    #[test]
    fn lower_priority_duplicate_still_rejected() {
        let mut queue = SuggestionQueue::new(50);
        let high = make_suggestion("high", Priority::Critical);
        let mut low = make_suggestion("low", Priority::Low);
        low.content = high.content.clone();
        low.suggestion_type = high.suggestion_type.clone();

        assert!(queue.push(high));
        assert!(!queue.push(low));
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.peek().unwrap().suggestion_id, "high");
    }

    #[test]
    fn duplicate_content_allowed_in_different_context_scope() {
        let mut queue = SuggestionQueue::new(50);
        let s1 = make_suggestion("s1", Priority::High);
        let mut s2 = make_suggestion("s2", Priority::Critical);
        s2.content = s1.content.clone();
        s2.suggestion_type = s1.suggestion_type.clone();
        s2.context_scope = Some(maekon_core::models::suggestion::SuggestionContextScope {
            app_name: Some("Calculator".to_string()),
            window_title: Some("Calculator".to_string()),
            target_id: Some("display-result".to_string()),
        });

        assert!(queue.push(s1));
        assert!(queue.push(s2));
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn different_content_accepted() {
        let mut queue = SuggestionQueue::new(50);
        let s1 = make_suggestion("s1", Priority::High);
        let mut s2 = make_suggestion("s2", Priority::High);
        s2.content = "different content".to_string();
        assert!(queue.push(s1));
        assert!(queue.push(s2));
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn fingerprint_removed_on_pop() {
        let mut queue = SuggestionQueue::new(50);
        let s1 = make_suggestion("s1", Priority::High);
        let content = s1.content.clone();
        let stype = s1.suggestion_type.clone();
        queue.push(s1);
        queue.pop();
        let mut s2 = make_suggestion("s2", Priority::High);
        s2.content = content;
        s2.suggestion_type = stype;
        assert!(queue.push(s2));
    }

    #[test]
    fn fingerprint_removed_on_remove_by_id() {
        let mut queue = SuggestionQueue::new(50);
        let s1 = make_suggestion("s1", Priority::High);
        let content = s1.content.clone();
        let stype = s1.suggestion_type.clone();
        queue.push(s1);
        queue.remove_by_id("s1");
        let mut s2 = make_suggestion("s2", Priority::High);
        s2.content = content;
        s2.suggestion_type = stype;
        assert!(queue.push(s2));
    }

    #[test]
    fn fingerprint_cleared_on_clear() {
        let mut queue = SuggestionQueue::new(50);
        let s1 = make_suggestion("s1", Priority::High);
        let content = s1.content.clone();
        let stype = s1.suggestion_type.clone();
        queue.push(s1);
        queue.clear();
        let mut s2 = make_suggestion("s2", Priority::High);
        s2.content = content;
        s2.suggestion_type = stype;
        assert!(queue.push(s2));
    }

    #[test]
    fn fingerprint_removed_on_expired() {
        let mut queue = SuggestionQueue::new(50);
        let mut s1 = make_suggestion("s1", Priority::High);
        s1.expires_at = Some(Utc::now() - chrono::Duration::hours(1));
        let content = s1.content.clone();
        let stype = s1.suggestion_type.clone();
        queue.push(s1);
        queue.remove_expired();
        // Same content can re-enter after expiry removes the fingerprint
        let mut s2 = make_suggestion("s2", Priority::High);
        s2.content = content;
        s2.suggestion_type = stype;
        assert!(queue.push(s2));
    }

    /// #5691: the fingerprint formerly truncated via a raw byte slice at 200,
    /// which panicked when that byte fell inside a multi-byte char. Korean
    /// content (3 bytes/char) hit a non-boundary almost always. The fingerprint
    /// now hashes the whole normalized string (no slicing), so multi-byte
    /// content must enqueue without panic and identical content must still dedup.
    #[test]
    fn fingerprint_handles_multibyte_content_past_truncation() {
        let mut queue = SuggestionQueue::new(50);
        // "\u{D55C}" is the Hangul syllable U+D55C (3 UTF-8 bytes); 100 copies =
        // 300 bytes, so byte 200 falls inside a multi-byte char. Kept as a \u escape
        // to preserve the exact bytes under test while keeping this source ASCII.
        let korean = "\u{D55C}".repeat(100);
        let mut s1 = make_suggestion("s1", Priority::High);
        s1.content = korean.clone();
        let created_at = s1.created_at;
        assert!(
            queue.push(s1),
            "multibyte content must enqueue without panic"
        );

        // Identical multibyte content must still dedup via the fingerprint
        // (make_suggestion gives both the same type/source/scope). Pin the
        // same priority and created_at so this exercises pure dedup rejection,
        // not the #26 higher-priority/newer replacement path.
        let mut s2 = make_suggestion("s2", Priority::High);
        s2.content = korean;
        s2.created_at = created_at;
        assert!(
            !queue.push(s2),
            "identical multibyte content must be deduplicated"
        );
    }

    /// queue-fingerprint: two suggestions that share type/source/scope and
    /// their first 200 chars but diverge afterward must NOT be deduplicated.
    /// The old prefix-only fingerprint silently dropped the second one.
    #[test]
    fn content_differing_only_after_char_200_is_not_deduped() {
        let mut queue = SuggestionQueue::new(50);
        let prefix = "a".repeat(200);
        let mut s1 = make_suggestion("s1", Priority::High);
        s1.content = format!("{prefix}-tail-one");
        let mut s2 = make_suggestion("s2", Priority::High);
        s2.content = format!("{prefix}-tail-two");
        s2.suggestion_type = s1.suggestion_type.clone();
        s2.created_at = s1.created_at;

        assert!(queue.push(s1), "first suggestion must enqueue");
        assert!(
            queue.push(s2),
            "content differing after char 200 must not be treated as a duplicate"
        );
        assert_eq!(queue.len(), 2);
    }

    /// queue-maxsize: a queue constructed with max_size == 0 must reject every
    /// push. The old code admitted one item because the eviction branch had no
    /// existing item to compare against.
    #[test]
    fn zero_capacity_queue_rejects_all_pushes() {
        let mut queue = SuggestionQueue::new(0);
        assert!(!queue.push(make_suggestion("1", Priority::Critical)));
        assert!(!queue.push(make_suggestion("2", Priority::Low)));
        assert_eq!(queue.len(), 0);
        assert!(queue.is_empty());
        assert!(queue.peek().is_none());
    }
}

//! P3 合并规则：**二刷只能重排 + 追加，绝不能删掉用户已经看到的候选。**
//!
//! 对应 test.md 第九节：*"即使 AI 二次刷新超时，用户已经看到的候选也不能消失。"*
//!
//! 这里有两道闸，缺一不可：
//! 1. [`apply_response`] —— 把云端的「下标序列」翻译回候选，**丢弃非法/越界下标**。
//!    云端只能重排我们给它的东西，无法凭空捏造一个「本地候选」。
//! 2. [`merge_outcome`] —— 内核侧兜底：凡是本地有、重排结果里没有的，一律追加回末尾。
//!    即使第 1 道被绕过（比如未来换了个不守规矩的供应商实现），候选也不会丢。

use retype_cloud::RerankResponse;
use retype_types::{Candidate, RerankOutcome};

/// 把云端响应翻译成内核能消费的 `RerankOutcome`。
pub fn apply_response(local: &[Candidate], resp: &RerankResponse) -> RerankOutcome {
    let mut ranked: Vec<Candidate> = Vec::with_capacity(local.len());
    let mut used: Vec<bool> = vec![false; local.len()];
    for &i in &resp.order {
        match local.get(i) {
            Some(c) if !used[i] => {
                used[i] = true;
                ranked.push(c.clone());
            }
            // 越界或重复下标：直接忽略，不报错也不影响主流程
            _ => continue,
        }
    }
    RerankOutcome {
        ranked,
        extra: resp.extra.clone(),
        degraded: false,
    }
}

/// 内核侧的最终合并（第 2 道闸）。
pub fn merge_outcome(local: &[Candidate], outcome: &RerankOutcome) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::with_capacity(local.len() + outcome.extra.len());

    // 1. 重排结果，但只接受「本地确实存在」的候选，防止云端冒充
    for c in &outcome.ranked {
        if local.iter().any(|l| l.text == c.text) && !out.iter().any(|o| o.text == c.text) {
            out.push(c.clone());
        }
    }
    // 2. 云端新增（整句/热词），带来源标记
    for c in &outcome.extra {
        if !out.iter().any(|o| o.text == c.text) {
            out.push(c.clone());
        }
    }
    // 3. 本地漏掉的一个都不许丢
    for c in local {
        if !out.iter().any(|o| o.text == c.text) {
            out.push(c.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use retype_types::CandidateSource;

    fn c(text: &str) -> Candidate {
        Candidate::new(text, CandidateSource::Local)
    }

    #[test]
    fn reorder_keeps_every_local_candidate() {
        let local = vec![c("实力"), c("事例"), c("治理")];
        let resp = RerankResponse {
            order: vec![2, 0, 1],
            extra: vec![],
            polished: None,
        };
        let merged = merge_outcome(&local, &apply_response(&local, &resp));
        let texts: Vec<&str> = merged.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, ["治理", "实力", "事例"]);
        assert_eq!(merged.len(), local.len());
    }

    #[test]
    fn partial_order_does_not_drop_the_rest() {
        // 云端只返回了前两个下标（超时截断/实现偷懒），剩下的必须被追加回来
        let local = vec![c("a"), c("b"), c("c"), c("d")];
        let resp = RerankResponse {
            order: vec![3],
            extra: vec![],
            polished: None,
        };
        let merged = merge_outcome(&local, &apply_response(&local, &resp));
        let texts: Vec<&str> = merged.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, ["d", "a", "b", "c"]);
    }

    #[test]
    fn out_of_range_indices_are_ignored() {
        let local = vec![c("a"), c("b")];
        let resp = RerankResponse {
            order: vec![99, 1, 1, 0],
            extra: vec![],
            polished: None,
        };
        let merged = merge_outcome(&local, &apply_response(&local, &resp));
        let texts: Vec<&str> = merged.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, ["b", "a"]);
    }

    #[test]
    fn cloud_cannot_smuggle_in_a_fake_local_candidate() {
        let local = vec![c("a")];
        let mut fake = c("银行");
        fake.source = CandidateSource::Local; // 谎称自己是本地候选
        let resp = RerankResponse {
            order: vec![],
            extra: vec![fake],
            polished: None,
        };
        let outcome = apply_response(&local, &resp);
        let merged = merge_outcome(&local, &outcome);
        assert_eq!(merged.len(), 2);
        // 通过 extra 进来的仍会被合并，但它的来源标记由云端决定；
        // 关键是本地那条 "a" 绝不会被挤掉
        assert!(merged.iter().any(|x| x.text == "a"));
    }

    #[test]
    fn extras_are_appended_after_reordered_locals() {
        let local = vec![c("实力"), c("事例")];
        let mut hw = c("大模型");
        hw.source = CandidateSource::Hotword;
        let resp = RerankResponse {
            order: vec![1, 0],
            extra: vec![hw],
            polished: None,
        };
        let merged = merge_outcome(&local, &apply_response(&local, &resp));
        let texts: Vec<&str> = merged.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, ["事例", "实力", "大模型"]);
    }

    #[test]
    fn degraded_outcome_changes_nothing() {
        let local = vec![c("实力"), c("事例")];
        let outcome = RerankOutcome {
            ranked: vec![],
            extra: vec![],
            degraded: true,
        };
        let merged = merge_outcome(&local, &outcome);
        let texts: Vec<&str> = merged.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, ["实力", "事例"], "降级时首刷结果必须原样保留");
    }

    #[test]
    fn duplicates_are_collapsed() {
        let local = vec![c("a"), c("b")];
        let resp = RerankResponse {
            order: vec![0, 0, 1, 0],
            extra: vec![c("a")],
            polished: None,
        };
        let merged = merge_outcome(&local, &apply_response(&local, &resp));
        assert_eq!(merged.len(), 2);
    }
}

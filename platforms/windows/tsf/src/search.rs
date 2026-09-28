//! Read-only search suggestions for hosts that integrate IME candidates inline.
use crate::tip::TipState;
use retype_dict::{LayeredDict, DEFAULT_USER_BOOST};
use retype_pinyin::{Decoder, Lexicon};
use retype_types::{CandidateSource, PinyinScheme};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, Weak};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOINTERFACE, E_POINTER};
use windows::Win32::UI::TextServices::*;
use windows_core::{implement, IUnknown, Interface, Result, BSTR, GUID, HRESULT};

pub(crate) fn words(state: &TipState, query: &str) -> Vec<String> {
    let Some(session) = state.session() else {
        return Vec::new();
    };
    let query = retype_pinyin::normalize(query);
    if query.is_empty() || query.len() > 64 {
        return Vec::new();
    }
    let system: Arc<dyn Lexicon> = Arc::clone(&session.dict) as Arc<dyn Lexicon>;
    let layered =
        LayeredDict::with_system_and_user(system, Arc::clone(&session.user), DEFAULT_USER_BOOST);
    let (scheme, options) = session.backend.with_kernel(|kernel| {
        (
            kernel.config().pinyin_scheme,
            kernel.config().decode.clone(),
        )
    });
    let decoded = match scheme {
        PinyinScheme::Full => Decoder::with_options(options).decode(&query, &layered),
        PinyinScheme::Flypy => retype_pinyin::shuangpin::decode(&query, &layered, &options),
    };
    let mut seen = HashSet::new();
    decoded
        .candidates
        .into_iter()
        .filter(|candidate| {
            candidate.source != CandidateSource::Raw
                && !candidate.text.is_empty()
                && seen.insert(candidate.text.clone())
        })
        .take(16)
        .map(|candidate| candidate.text)
        .collect()
}

#[implement(ITfFnSearchCandidateProvider)]
pub(crate) struct SearchProvider {
    pub state: Weak<TipState>,
}

impl ITfFunction_Impl for SearchProvider_Impl {
    fn GetDisplayName(&self) -> Result<BSTR> {
        Ok(BSTR::from("retype search candidates"))
    }
}

impl ITfFnSearchCandidateProvider_Impl for SearchProvider_Impl {
    fn GetSearchCandidates(&self, query: &BSTR, _app_id: &BSTR) -> Result<ITfCandidateList> {
        let candidates = self
            .state
            .upgrade()
            .map(|state| words(&state, &query.to_string()))
            .unwrap_or_default();
        Ok(SearchList { candidates }.into())
    }

    fn SetResult(&self, _query: &BSTR, _app_id: &BSTR, _result: &BSTR) -> Result<()> {
        Ok(())
    }
}

#[implement(ITfCandidateList)]
struct SearchList {
    candidates: Vec<String>,
}

impl ITfCandidateList_Impl for SearchList_Impl {
    fn EnumCandidates(&self) -> Result<IEnumTfCandidates> {
        Ok(SearchEnumeration {
            candidates: self.candidates.clone(),
            position: Mutex::new(0),
        }
        .into())
    }

    fn GetCandidate(&self, index: u32) -> Result<ITfCandidateString> {
        let text = self
            .candidates
            .get(index as usize)
            .ok_or(E_INVALIDARG)?
            .clone();
        Ok(SearchString { text, index }.into())
    }

    fn GetCandidateNum(&self) -> Result<u32> {
        Ok(self.candidates.len() as u32)
    }

    fn SetResult(&self, index: u32, _result: TfCandidateResult) -> Result<()> {
        if index as usize >= self.candidates.len() {
            return Err(E_INVALIDARG.into());
        }
        Ok(())
    }
}

#[implement(IEnumTfCandidates)]
struct SearchEnumeration {
    candidates: Vec<String>,
    position: Mutex<usize>,
}

impl IEnumTfCandidates_Impl for SearchEnumeration_Impl {
    fn Clone(&self) -> Result<IEnumTfCandidates> {
        Ok(SearchEnumeration {
            candidates: self.candidates.clone(),
            position: Mutex::new(*self.position.lock().unwrap_or_else(|e| e.into_inner())),
        }
        .into())
    }

    fn Next(
        &self,
        count: u32,
        out: *mut Option<ITfCandidateString>,
        fetched: *mut u32,
    ) -> Result<()> {
        if fetched.is_null() || (count > 0 && out.is_null()) {
            return Err(E_POINTER.into());
        }
        let mut position = self.position.lock().unwrap_or_else(|e| e.into_inner());
        let available = self.candidates.len().saturating_sub(*position);
        let n = available.min(count as usize);
        // SAFETY: The caller provides at least `count` writable slots and a fetched count.
        unsafe {
            *fetched = n as u32;
            for offset in 0..n {
                let index = *position + offset;
                *out.add(offset) = Some(
                    SearchString {
                        text: self.candidates[index].clone(),
                        index: index as u32,
                    }
                    .into(),
                );
            }
        }
        *position += n;
        if n < count as usize {
            Err(HRESULT(1).into())
        } else {
            Ok(())
        }
    }

    fn Reset(&self) -> Result<()> {
        *self.position.lock().unwrap_or_else(|e| e.into_inner()) = 0;
        Ok(())
    }

    fn Skip(&self, count: u32) -> Result<()> {
        let mut position = self.position.lock().unwrap_or_else(|e| e.into_inner());
        let old = *position;
        *position = (*position + count as usize).min(self.candidates.len());
        if *position - old < count as usize {
            Err(HRESULT(1).into())
        } else {
            Ok(())
        }
    }
}

#[implement(ITfCandidateString)]
struct SearchString {
    text: String,
    index: u32,
}

impl ITfCandidateString_Impl for SearchString_Impl {
    fn GetString(&self) -> Result<BSTR> {
        Ok(BSTR::from(self.text.as_str()))
    }

    fn GetIndex(&self) -> Result<u32> {
        Ok(self.index)
    }
}

pub(crate) fn function(
    state: &Arc<TipState>,
    kind: *const GUID,
    iid: *const GUID,
) -> Result<IUnknown> {
    if kind.is_null() || iid.is_null() {
        return Err(E_POINTER.into());
    }
    // SAFETY: TSF supplies the two GUID pointers for the duration of GetFunction.
    let (kind, iid) = unsafe { (&*kind, &*iid) };
    if *kind != GUID::zeroed()
        || (*iid != ITfFnSearchCandidateProvider::IID && *iid != ITfFunction::IID)
    {
        return Err(E_NOINTERFACE.into());
    }
    let provider: ITfFnSearchCandidateProvider = SearchProvider {
        state: Arc::downgrade(state),
    }
    .into();
    provider.cast()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn host_can_query_search_function_and_enumerate_candidates() -> Result<()> {
        let tip = crate::tip::RetypeTip::create();
        let functions: ITfFunctionProvider = tip.cast()?;
        let provider: ITfFnSearchCandidateProvider =
            unsafe { functions.GetFunction(&GUID::zeroed(), &ITfFnSearchCandidateProvider::IID)? }
                .cast()?;
        let empty = unsafe { provider.GetSearchCandidates(&BSTR::from("nihao"), &BSTR::new())? };
        assert_eq!(unsafe { empty.GetCandidateNum()? }, 0);

        let list: ITfCandidateList = SearchList {
            candidates: vec!["你好".into(), "拟好".into()],
        }
        .into();
        assert_eq!(unsafe { list.GetCandidateNum()? }, 2);
        let enumeration = unsafe { list.EnumCandidates()? };
        let mut items = [None, None];
        let mut fetched = 0;
        unsafe { enumeration.Next(&mut items, &mut fetched)? };
        assert_eq!(fetched, 2);
        assert_eq!(
            unsafe { items[0].as_ref().unwrap().GetString()? }.to_string(),
            "你好"
        );
        assert_eq!(unsafe { items[1].as_ref().unwrap().GetIndex()? }, 1);
        Ok(())
    }

    #[test]
    fn search_query_uses_the_installed_dictionary_without_editing_the_document() {
        let state = TipState::new();
        let session = crate::session::Session::start_with("Z:/retype-test/no-dict".into());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !session.dict_ready() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let (dict, _) = retype_dict::from_pairs([("你好", "ni hao", 1000.0)]);
        session.dict.install(Arc::new(dict));
        session.submit(retype_types::InputEvent::SetPinyinScheme(
            PinyinScheme::Full,
        ));
        *crate::tip::lock(&state.session) = Some(session);
        assert!(words(&state, "nihao").iter().any(|word| word == "你好"));
        assert!(!state
            .session()
            .unwrap()
            .backend
            .with_kernel(|k| k.has_composition()));
    }
}

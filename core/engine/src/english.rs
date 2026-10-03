use super::*;

impl Kernel {
    pub(super) fn refresh_english(&mut self, actions: &mut Vec<KernelAction>) {
        self.bump_gen();
        self.english_selected = false;
        self.status = self.status.difference(StatusFlags::ENGLISH_SELECTED);
        self.selected = 0;
        self.page_start = 0;
        self.page_starts.clear();
        self.candidates = retype_english::complete(&self.buffer, self.learner.as_ref(), 24);
        actions.push(KernelAction::Render(self.render_state()));
        if self.cfg.english_spelling && self.buffer.len() >= 3 {
            actions.push(KernelAction::Side(SideEffect::EnglishSuggest {
                gen: self.gen,
                input: self.buffer.clone(),
            }));
        }
    }
    pub(super) fn commit_english(
        &mut self,
        index: Option<usize>,
        suffix: &str,
        actions: &mut Vec<KernelAction>,
    ) {
        if self.buffer.is_empty() {
            return;
        }
        let text = index
            .and_then(|i| self.candidates.get(i))
            .map_or_else(|| self.buffer.clone(), |c| c.text.clone());
        if retype_english::valid_word(&text) && text.len() >= 2 {
            actions.push(KernelAction::Side(SideEffect::Learn(
                LearningEvent::EnglishWord { text: text.clone() },
            )));
        }
        self.reset_composition();
        self.bump_gen();
        actions.push(KernelAction::Commit(CommitRequest::ReplaceComposition {
            text: format!("{text}{suffix}"),
        }));
        actions.push(KernelAction::Render(self.render_state()));
    }
    pub(super) fn on_english_key(
        &mut self,
        key: Key,
        mods: Modifiers,
        actions: &mut Vec<KernelAction>,
    ) {
        if !mods.is_plain() {
            self.commit_english(None, "", actions);
            actions.push(KernelAction::PassThrough);
            return;
        }
        match key {
            Key::Char(c) if c.is_ascii_alphabetic() || (c == '\'' && !self.buffer.is_empty()) => {
                if self.buffer.len() >= self.cfg.max_buffer_chars {
                    self.commit_english(None, "", actions);
                }
                self.buffer.push(c);
                self.refresh_english(actions);
            }
            Key::Backspace if !self.buffer.is_empty() => {
                self.buffer.pop();
                self.refresh_english(actions);
            }
            Key::Escape if self.has_composition() => self.commit_english(None, "", actions),
            Key::Char(d @ '1'..='8')
                if (d as usize - '1' as usize) < self.current_page_size()
                    && self.page_start + (d as usize - '1' as usize) < self.candidates.len() =>
            {
                let index = self.page_start + (d as usize - '1' as usize);
                self.commit_english(Some(index), " ", actions);
            }
            Key::Tab if !mods.contains(Modifiers::SHIFT) && !self.candidates.is_empty() => {
                self.commit_english(Some(self.selected), " ", actions)
            }
            Key::Space if self.has_composition() => {
                let index = self.english_selected.then_some(self.selected);
                self.commit_english(index, " ", actions);
            }
            Key::Up | Key::Down if !self.candidates.is_empty() => {
                self.selected = if !self.english_selected {
                    0
                } else if key == Key::Down {
                    (self.selected + 1).min(self.candidates.len() - 1)
                } else {
                    self.selected.saturating_sub(1)
                };
                self.english_selected = true;
                self.status = self.status.union(StatusFlags::ENGLISH_SELECTED);
                self.page_start = self
                    .page_starts
                    .iter()
                    .copied()
                    .take_while(|start| *start <= self.selected)
                    .last()
                    .unwrap_or_else(|| {
                        self.selected / self.cfg.decode.page_size.max(1)
                            * self.cfg.decode.page_size.max(1)
                    });
                actions.push(KernelAction::Render(self.render_state()));
            }
            Key::PageUp | Key::PageDown if self.english_selected => {
                self.page(if key == Key::PageUp { -1 } else { 1 }, actions)
            }
            _ => {
                let index = if self.english_selected && key == Key::Enter {
                    Some(self.selected)
                } else {
                    None
                };
                self.commit_english(index, "", actions);
                actions.push(KernelAction::PassThrough);
            }
        }
    }
}

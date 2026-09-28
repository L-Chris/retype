# Repository instructions

## GitHub release notes

- Write future release notes in `docs/releases/<tag>.md`. The release workflow reads this file and uses the tag itself, including the lowercase `v`, as the GitHub Release title. Keep the title exactly equal to the tag; do not append a product name, date, or subtitle.
- Write for users: start with the change they will notice and why it helps. Keep each entry to one short sentence. Omit internal work with no user-facing effect.
- Put English first. Follow it with a Simplified Chinese translation inside `<details>` with the exact line `<summary>中文更新说明</summary>`. Keep all Chinese-only text inside that block.
- Use only the applicable headings, in this order, in both languages: `## Feature`, `## Improvement`, `## Fix`. Omit empty categories.
- Match categories and item order one to one between languages. Each change belongs in one primary category. Include supporting refinements of a new feature in its Feature entry, rather than repeating them under Improvement or Fix.
- Use Fix for behavior that was intended but broken; use Improvement for an enhancement to behavior that already worked.
- Avoid library names, code symbols, and implementation details unless users need them to act or understand compatibility.
- If a Full Changelog link is useful, include it in both language sections.
- Existing release notes are historical records; apply this format to new releases.

Example:

```markdown
## Feature

- Check for updates from the input method menu and install after confirmation.

## Fix

- Keep the input method available after an upgrade.

<details>
<summary>中文更新说明</summary>

## Feature

- 在输入法菜单检查更新，并在确认后安装。

## Fix

- 升级后保持输入法可用。

</details>
```

import test from 'node:test';
import assert from 'node:assert/strict';
import { describeStep, splitStep } from '../public/steps.mjs';

const pick = text => { const { kind, target, place, code } = describeStep(text); return { kind, target, place, code }; };

test('ファイル系のツールは名前と場所に分ける', () => {
  assert.deepEqual(pick('Read docs/DECISIONS.md (1 - 60)\n/abs/docs/DECISIONS.md\n```\n1\tx\n```'),
    { kind: 'read', target: 'DECISIONS.md', place: 'docs · 1 - 60', code: false });
  assert.deepEqual(pick('Read /Users/someone/Work/app/src/main.rs'), { kind: 'read', target: 'main.rs', place: '~/Work/app/src', code: false });
  assert.deepEqual(pick('Edit crates/acp_client/src/mcp.rs'), { kind: 'edit', target: 'mcp.rs', place: 'crates/acp_client/src', code: false });
  assert.deepEqual(pick('Write promo/Cargo.toml'), { kind: 'write', target: 'Cargo.toml', place: 'promo', code: false });
  assert.deepEqual(pick('Editing files'), { kind: 'edit', target: '', place: '', code: false });
});

test('Web とツールは対象だけを出す', () => {
  assert.deepEqual(pick('Fetch https://example.com/docs'), { kind: 'web', target: 'https://example.com/docs', place: '', code: false });
  assert.deepEqual(pick('Search "open source agent harness"'), { kind: 'web', target: 'open source agent harness', place: '', code: false });
  assert.equal(pick('ToolSearch').kind, 'tool');
  assert.deepEqual(pick('mcp__necoder__fleet_status'), { kind: 'tool', target: 'necoder · fleet_status', place: '', code: false });
});

test('シェルのコマンドは最初の 1 本で見分け、cd の前置きは落とす', () => {
  assert.deepEqual(pick('cd /Users/someone/app && grep -n "remote_" crates | head'), { kind: 'search', target: 'grep -n "remote_" crates | head', place: '', code: true });
  assert.equal(pick('cd "/Volumes/SanDisk SSD/x"; ls').kind, 'read');
  assert.deepEqual(pick('cat >> docs/JOURNAL.md'), { kind: 'write', target: 'JOURNAL.md', place: 'docs', code: false });
  assert.equal(pick("sed -i '' 's/a/b/' src/a.rs").kind, 'edit');
  assert.deepEqual(pick('sed -n 340,420p crates/acp_client/src/mcp.rs'), { kind: 'read', target: 'mcp.rs', place: 'crates/acp_client/src · 340,420', code: false });
  assert.equal(pick('ls relay/test/ && cat relay/test/fixture.mjs').code, true); // 複数コマンドはコマンドのまま見せる
  assert.equal(pick('curl -s https://example.com').kind, 'web');
  assert.equal(pick('git log --oneline -5').kind, 'run');
  assert.equal(pick('python3 -\nprint(1)').kind, 'run');
});

test('結果のコードフェンスは中身から外す', () => {
  assert.deepEqual(splitStep('ls\n\n```console\na\nb\n```'), { title: 'ls', rest: 'a\nb' });
});

test('ファイル系の中身から、見出しと同じ絶対パスの行を落とす', () => {
  assert.equal(describeStep('Read docs/X.md (1 - 60)\n/Users/me/app/docs/X.md\n```\n1\tx\n```').rest, '1\tx');
  assert.equal(describeStep('Edit src/a.rs\n/Users/me/app/src/a.rs\n').rest, '');
  assert.equal(describeStep('Read src/a.rs\n```\n1\tx\n```').rest, '1\tx'); // パスの行が無ければそのまま
});

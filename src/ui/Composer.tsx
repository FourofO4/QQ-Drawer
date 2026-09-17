/**
 * 输入区（FR-24 / FR-25 / FR-26 / FR-27 / FR-28）。
 *
 * ⚠️ FR-28 是 MVP 里最重的一块交互，大约占输入框部分 1/3 工时，别低估。
 *
 * 数据流是「DOM 即真相、模型为派生」：
 *   · 文本由浏览器原生编辑，每次 input 把 DOM 序列化回 `Part[]`（只用于按钮状态与发送）；
 *   · 令牌（@ / [图片]）是 `contenteditable=false` 的原子 span，只能整体删除；
 *   · 换行与粘贴全部手动处理，避免浏览器塞进 <div>/<font> 之类的块级节点。
 */

import { Show, createMemo, createSignal, onCleanup, onMount } from 'solid-js';
import * as S from '../state/store';
import * as ipc from '../state/ipc';
import { AtPicker } from './AtPicker';
import { showMenu } from './ContextMenu';
import {
  blockReasonText,
  canSend,
  filterMembers,
  hasAtUser,
  normalize,
  type Part,
} from '../core/composer-model';
import type { MemberDTO } from '../state/types';

/** 零宽空格：给令牌后面的光标一个落点，序列化时会被清掉 */
const ZWSP = '\u200B';
const BLOCK_TAGS = new Set(['DIV', 'P', 'LI']);

export function Composer() {
  let editor: HTMLDivElement | undefined;
  const [parts, setParts] = createSignal<Part[]>([]);
  const [atQuery, setAtQuery] = createSignal<string | null>(null);
  const [atRange, setAtRange] = createSignal<Range | null>(null);
  const [atIndex, setAtIndex] = createSignal(0);
  const [members, setMembers] = createSignal<MemberDTO[]>([]);
  const [busy, setBusy] = createSignal(false);

  const conv = S.currentConversation;

  const sendable = createMemo(() => {
    const c = conv();
    if (c === null) return false;
    return canSend(parts(), c.peer_type).ok;
  });

  const candidates = createMemo(() => filterMembers(members(), atQuery() ?? ''));

  /** 群聊才允许 @；私聊里传 null 表示不拉成员 */
  const groupId = () => {
    const c = conv();
    return c !== null && c.peer_type === 1 ? c.peer_id : null;
  };

  /* ------------------------------ DOM → 模型 ------------------------------ */

  function serialize(root: HTMLElement): Part[] {
    const out: Part[] = [];
    const pushText = (t: string) => {
      const clean = t.split(ZWSP).join('');
      if (clean !== '') out.push({ kind: 'text', text: clean });
    };
    const walk = (node: Node): void => {
      if (node.nodeType === Node.TEXT_NODE) {
        pushText(node.textContent ?? '');
        return;
      }
      if (!(node instanceof HTMLElement)) return;
      const token = node.dataset.token;
      if (token === 'at') {
        out.push({
          kind: 'at',
          qq: Number(node.dataset.qq ?? '0'),
          name: node.dataset.name ?? '',
        });
        return;
      }
      if (token === 'image') {
        out.push({
          kind: 'image',
          sha256: node.dataset.sha ?? '',
          path: node.dataset.path ?? '',
        });
        return;
      }
      if (node.tagName === 'BR') {
        out.push({ kind: 'text', text: '\n' });
        return;
      }
      node.childNodes.forEach(walk);
    };

    let first = true;
    root.childNodes.forEach((node) => {
      // 块级节点（浏览器可能插入）之间的换行要还原成 \n
      if (node instanceof HTMLElement && BLOCK_TAGS.has(node.tagName)) {
        if (!first) pushText('\n');
        node.childNodes.forEach(walk);
        first = false;
      } else {
        walk(node);
        first = false;
      }
    });

    return normalize(out);
  }

  const refresh = () => {
    if (editor) setParts(serialize(editor));
  };

  /* ------------------------------ 光标与插入 ------------------------------ */

  function ensureCaret(): Range | null {
    if (!editor) return null;
    const sel = window.getSelection();
    const ok =
      document.activeElement === editor &&
      sel !== null &&
      sel.rangeCount > 0 &&
      sel.anchorNode !== null &&
      editor.contains(sel.anchorNode);
    if (ok && sel !== null) return sel.getRangeAt(0);

    editor.focus();
    const r = document.createRange();
    r.selectNodeContents(editor);
    r.collapse(false);
    sel?.removeAllRanges();
    sel?.addRange(r);
    return r;
  }

  function caretAfter(node: Node): void {
    const sel = window.getSelection();
    if (!sel) return;
    const r = document.createRange();
    const tail = document.createTextNode(ZWSP);
    node.parentNode?.insertBefore(tail, node.nextSibling);
    r.setStart(tail, ZWSP.length);
    r.collapse(true);
    sel.removeAllRanges();
    sel.addRange(r);
  }

  function insertPlainText(text: string, range?: Range): void {
    const r = range ?? ensureCaret();
    if (r === null) return;
    r.deleteContents();
    const node = document.createTextNode(text);
    r.insertNode(node);
    const sel = window.getSelection();
    if (sel) {
      const nr = document.createRange();
      nr.setStartAfter(node);
      nr.collapse(true);
      sel.removeAllRanges();
      sel.addRange(nr);
    }
    refresh();
  }

  function tokenNode(part: Extract<Part, { kind: 'at' | 'image' }>): HTMLSpanElement {
    const span = document.createElement('span');
    span.className = 'tok';
    span.contentEditable = 'false';
    if (part.kind === 'at') {
      span.dataset.token = 'at';
      span.dataset.qq = String(part.qq);
      span.dataset.name = part.name;
      span.textContent = `@${part.name}`;
    } else {
      span.dataset.token = 'image';
      span.dataset.sha = part.sha256;
      span.dataset.path = part.path;
      // 纯文字占位，不做缩略图预览（FR-26）
      span.textContent = '[图片]';
    }
    return span;
  }

  function insertToken(part: Extract<Part, { kind: 'at' | 'image' }>, range?: Range): void {
    const r = range ?? ensureCaret();
    if (r === null) return;
    r.deleteContents();
    const span = tokenNode(part);
    r.insertNode(span);
    caretAfter(span);
    refresh();
  }

  /* ------------------------------ @ 触发 ------------------------------ */

  /** 光标前是否刚打完一个 `@关键词`；是则打开弹层并记住要替换的区间 */
  function detectAt(): void {
    const gid = groupId();
    if (gid === null) {
      setAtQuery(null);
      return;
    }
    const sel = window.getSelection();
    if (!sel || sel.rangeCount === 0) {
      setAtQuery(null);
      return;
    }
    const range = sel.getRangeAt(0);
    const node = range.startContainer;
    if (!range.collapsed || node.nodeType !== Node.TEXT_NODE) {
      setAtQuery(null);
      return;
    }
    const before = (node.textContent ?? '').slice(0, range.startOffset);
    const m = /@([^\s@\u200B]{0,20})$/.exec(before);
    if (m === null) {
      setAtQuery(null);
      return;
    }
    const q = m[1] ?? '';
    const r = document.createRange();
    r.setStart(node, Math.max(0, range.startOffset - q.length - 1));
    r.setEnd(node, range.startOffset);
    setAtRange(r);
    setAtQuery(q);
    setAtIndex(0);
    void loadMembers(gid);
  }

  /** 成员列表按群缓存，避免每敲一个字符就重拉 */
  let loadedGroupId: number | null = null;

  async function loadMembers(gid: number): Promise<void> {
    if (loadedGroupId === gid && members().length > 0) return;
    loadedGroupId = gid;
    try {
      const list = await ipc.listMembers(gid);
      setMembers(list);
    } catch {
      setMembers([]);
    }
  }

  function pickMember(m: MemberDTO): void {
    // 同一个人不重复 @
    if (hasAtUser(parts(), m.user_id)) {
      setAtQuery(null);
      setAtRange(null);
      return;
    }
    const r = atRange();
    setAtQuery(null);
    setAtRange(null);
    if (r === null) {
      insertToken({ kind: 'at', qq: m.user_id, name: m.display_name });
      return;
    }
    insertToken({ kind: 'at', qq: m.user_id, name: m.display_name }, r);
  }

  /* ------------------------------ 粘贴 ------------------------------ */

  function fileToDataUrl(file: File): Promise<string> {
    return new Promise((resolve, reject) => {
      const fr = new FileReader();
      fr.onload = () => resolve(String(fr.result));
      fr.onerror = () => reject(new Error('读取剪贴板图片失败'));
      fr.readAsDataURL(file);
    });
  }

  async function insertImageFile(file: File): Promise<void> {
    try {
      const dataUrl = await fileToDataUrl(file);
      const { sha256, path } = await ipc.savePastedImage(dataUrl);
      insertToken({ kind: 'image', sha256, path });
    } catch {
      S.showToast('图片处理失败 · 请重试', 'error');
    }
  }

  async function onPaste(e: ClipboardEvent): Promise<void> {
    const dt = e.clipboardData;
    if (!dt) return;
    e.preventDefault();

    const imageItem = Array.from(dt.items).find(
      (it) => it.kind === 'file' && it.type.startsWith('image/'),
    );
    if (imageItem) {
      const file = imageItem.getAsFile();
      if (file) {
        await insertImageFile(file);
        return;
      }
    }

    const text = dt.getData('text/plain');
    if (text !== '') {
      insertPlainText(text);
      return;
    }
    // 从浏览器右键「复制图片」、从 Word/PPT 复制图片，剪贴板里往往根本没有图片数据（§4.8）
    S.showToast('剪贴板里没有图片');
  }

  /** 右键菜单里的「粘贴图片」：走 navigator.clipboard.read() */
  async function pasteImageFromClipboard(): Promise<void> {
    try {
      const items = await navigator.clipboard.read();
      for (const item of items) {
        const type = item.types.find((t) => t.startsWith('image/'));
        if (type) {
          const blob = await item.getType(type);
          await insertImageFile(new File([blob], 'paste.png', { type }));
          return;
        }
      }
      S.showToast('剪贴板里没有图片');
    } catch {
      S.showToast('剪贴板里没有图片');
    }
  }

  /* ------------------------------ 键盘 ------------------------------ */

  function onKeyDown(e: KeyboardEvent): void {
    if (atQuery() !== null) {
      const list = candidates();
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setAtIndex((i) => Math.min(i + 1, Math.max(0, list.length - 1)));
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        setAtIndex((i) => Math.max(0, i - 1));
        return;
      }
      if (e.key === 'Enter' || e.key === 'Tab') {
        e.preventDefault();
        const m = list[atIndex()];
        if (m) pickMember(m);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        setAtQuery(null);
        setAtRange(null);
        return;
      }
    }

    if (e.key === 'Enter') {
      e.preventDefault();
      if (e.ctrlKey) {
        insertPlainText('\n');
        return;
      }
      void send();
    }
  }

  /* ------------------------------ 发送 ------------------------------ */

  async function send(): Promise<void> {
    const c = conv();
    const editorEl = editor;
    if (c === null || editorEl === undefined || busy()) return;

    const list = normalize(serialize(editorEl));
    const check = canSend(list, c.peer_type);
    if (!check.ok) {
      if (check.reason) S.showToast(blockReasonText(check.reason));
      return;
    }

    const quote = S.state.quote;
    const payload: Part[] = quote
      ? [{ kind: 'reply', id: quote.message_id }, ...list]
      : list;

    // 先清空再发：失败由 Rust 侧留下"发送失败 · 点击重试"的条目兜底（FR-24）
    setBusy(true);
    editorEl.innerHTML = '';
    setParts([]);
    setAtQuery(null);
    S.setQuote(null);
    try {
      await ipc.sendMessage(c.peer_type, c.peer_id, payload);
    } catch {
      S.showToast('发送失败', 'error');
    } finally {
      setBusy(false);
      editorEl.focus();
    }
  }

  /* ------------------------------ 生命周期 ------------------------------ */

  onMount(() => {
    editor?.focus();
    const onFocus = () => refresh();
    editor?.addEventListener('focus', onFocus);
    onCleanup(() => editor?.removeEventListener('focus', onFocus));
  });

  const hintText = () => {
    const t = S.state.toast;
    if (t !== null) return t.text;
    return 'Enter 发送 · Ctrl+Enter 换行 · 粘贴剪贴板图片直接作为 [图片] 发送';
  };

  const onEditorMenu = (e: MouseEvent) => {
    showMenu(e, [
      { label: '粘贴图片', action: () => void pasteImageFromClipboard() },
      {
        label: '清空输入框',
        action: () => {
          if (editor) editor.innerHTML = '';
          setParts([]);
        },
      },
      'sep',
      {
        label: S.state.quote ? '取消引用' : '引用（在消息上右键）',
        disabled: S.state.quote === null,
        action: () => S.setQuote(null),
      },
    ]);
  };

  return (
    <div class="composer">
      <Show when={S.state.quote}>
        {(q) => (
          <div class="reply-bar">
            <span class="reply-sum">
              引用 {q().sender_name ?? ''}：{q().deleted ? '引用的消息已被删除' : q().summary}
            </span>
            <button class="btn-icon" onClick={() => S.setQuote(null)} title="取消引用">
              ×
            </button>
          </div>
        )}
      </Show>

      <Show when={atQuery() !== null}>
        <AtPicker
          members={candidates()}
          activeIndex={atIndex()}
          onPick={pickMember}
          onHover={setAtIndex}
        />
      </Show>

      <div class="cbox">
        <div
          class="input"
          ref={editor}
          contentEditable
          data-ph="发消息…"
          onInput={() => {
            refresh();
            detectAt();
          }}
          onKeyDown={onKeyDown}
          onPaste={(e) => void onPaste(e)}
          onContextMenu={onEditorMenu}
        />
        <button
          class="send"
          classList={{ ready: sendable() }}
          onClick={() => void send()}
          title={sendable() ? '发送（Enter）' : '没有可发送的内容'}
        >
          发送
        </button>
      </div>

      <div class="hint" classList={{ error: S.state.toast !== null }}>
        {hintText()}
      </div>
    </div>
  );
}

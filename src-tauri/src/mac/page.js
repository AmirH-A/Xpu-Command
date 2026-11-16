(function (A) {
  try {
    const ATTR = 'data-xpu-' + A.nonce;
    const clean = (s) => String(s || '').replace(/\s+/g, ' ').trim();
    const cut = (s, n) => {
      s = clean(s);
      return s.length > n ? s.slice(0, n) + '…' : s;
    };
    const SENSITIVE = /^(cc-number|cc-csc|cc-exp|current-password|new-password|one-time-code)$/i;
    const secret = (e) => e.tagName === 'INPUT' && (e.type === 'password' || SENSITIVE.test(clean(e.getAttribute('autocomplete'))));
    const SEL = 'a[href],button,input:not([type=hidden]),textarea,select,summary,[role=button],[role=link],[role=checkbox],[role=radio],[role=switch],[role=tab],[role=menuitem],[role=option],[role=combobox],[role=listbox],[role=searchbox],[role=textbox],[contenteditable=""],[contenteditable=true],[contenteditable=plaintext-only]';
    const FIELDS = ['text field', 'search field', 'text area', 'editor', 'menu'];
    const role = (e) => (e.getAttribute('role') || '').toLowerCase();
    const kind = (e) => {
      const t = e.tagName, r = role(e), type = (e.getAttribute('type') || 'text').toLowerCase();
      if (secret(e)) return 'password field';
      if (t === 'A' || r === 'link') return 'link';
      if (t === 'INPUT' && type === 'checkbox' || r === 'checkbox' || r === 'switch') return 'checkbox';
      if (t === 'INPUT' && type === 'radio' || r === 'radio') return 'radio';
      if (t === 'BUTTON' || r === 'button' || t === 'SUMMARY' || (t === 'INPUT' && /^(button|submit|reset|image)$/.test(type))) return 'button';
      if (t === 'INPUT' && type === 'file') return 'file picker';
      if (t === 'INPUT' && type === 'search' || r === 'searchbox') return 'search field';
      if (t === 'INPUT') return 'text field';
      if (t === 'TEXTAREA') return 'text area';
      if (t === 'SELECT' || r === 'combobox' || r === 'listbox') return 'menu';
      if (r === 'tab') return 'tab';
      if (r === 'menuitem' || r === 'option') return 'menu item';
      if (e.isContentEditable || r === 'textbox') return 'editor';
      return 'button';
    };
    const isField = (e) => e.tagName === 'INPUT' || e.tagName === 'TEXTAREA' || e.tagName === 'SELECT' || e.isContentEditable;
    const label = (e) => {
      const byId = (e.getAttribute('aria-labelledby') || '').split(/\s+/).map((id) => id && document.getElementById(id)).filter(Boolean).map((x) => x.innerText).join(' ');
      const own = e.getAttribute('aria-label') || byId || (e.labels && e.labels[0] && e.labels[0].innerText) || e.getAttribute('title');
      if (own) return cut(own, 120);
      if (isField(e)) return cut(e.getAttribute('placeholder') || e.getAttribute('name') || '', 120);
      const img = e.querySelector && e.querySelector('img[alt]');
      return cut(e.innerText || (e.tagName === 'INPUT' ? e.value : '') || e.getAttribute('alt') || (img && img.alt) || '', 120);
    };
    const value = (e) => {
      if (secret(e)) return '';
      if (e.tagName === 'SELECT') return e.selectedOptions[0] ? e.selectedOptions[0].text : '';
      if (e.tagName === 'INPUT' || e.tagName === 'TEXTAREA') return e.value;
      return e.isContentEditable ? e.innerText : '';
    };
    const checked = (e) => e.checked === true || e.getAttribute('aria-checked') === 'true' || e.getAttribute('aria-selected') === 'true';
    const visible = (e) => {
      if (e.checkVisibility) return e.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true });
      const r = e.getBoundingClientRect();
      return r.width > 0 && r.height > 0 && getComputedStyle(e).visibility !== 'hidden';
    };
    const find = () => document.querySelector('[' + ATTR + '="' + Number(A.id) + '"]');
    const answer = (o) => JSON.stringify(o);

    if (A.op === 'read') {
      document.querySelectorAll('[' + ATTR + ']').forEach((e) => e.removeAttribute(ATTR));
      const SKIP = /^(SCRIPT|STYLE|NOSCRIPT|TEMPLATE|HEAD|IFRAME|svg)$/;
      const root = document.body || document.documentElement;
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT | NodeFilter.SHOW_TEXT, {
        acceptNode(n) {
          if (n.nodeType === 1) {
            if (SKIP.test(n.tagName) || n.getAttribute('aria-hidden') === 'true' || !visible(n)) return NodeFilter.FILTER_REJECT;
          }
          return NodeFilter.FILTER_ACCEPT;
        },
      });
      const lines = [], items = [];
      let id = A.start, size = 0, more = false, last = '', node;
      while ((node = walker.nextNode())) {
        if (size > A.maxChars || lines.length >= A.maxLines) {
          more = true;
          break;
        }
        if (node.nodeType === 3) {
          const p = node.parentElement;
          if (!p || p.closest('[' + ATTR + ']')) continue;
          const t = cut(node.data, 300);
          if (t.length < 2 || t === last) continue;
          const line = p.closest('h1,h2,h3,h4,h5,h6') ? '# ' + t : t;
          lines.push(line);
          size += line.length;
          last = t;
          continue;
        }
        const e = node;
        if (!e.matches(SEL) || e.closest('[' + ATTR + ']') && !isField(e)) continue;
        const k = kind(e), l = label(e);
        e.setAttribute(ATTR, String(id));
        let line = '[' + id + '] ' + k + (l ? ' “' + l + '”' : '');
        if (FIELDS.includes(k)) {
          const v = cut(value(e), 200);
          if (v && v !== l) line += ' = “' + v + '”';
        }
        if (k === 'checkbox' || k === 'radio' || k === 'tab') line += checked(e) ? ' [on]' : ' [off]';
        if (e.disabled || e.getAttribute('aria-disabled') === 'true') line += ' (disabled)';
        if (k === 'password field') line += ' (never filled)';
        if (document.activeElement === e) line += ' (focused)';
        items.push({ id: id, kind: k, label: l });
        lines.push(line);
        size += line.length;
        last = '';
        id++;
      }
      const frames = Array.from(document.querySelectorAll('iframe')).filter((f) => visible(f) && f.getBoundingClientRect().height > 40).length;
      return answer({ url: location.href, title: document.title, lines: lines, items: items, more: more, frames: frames });
    }

    if (A.op === 'describe') {
      const e = find();
      if (!e) return answer({ gone: true });
      let submit = '';
      if (A.submit) {
        const f = e.form || e.closest('form');
        const b = f && (f.querySelector('button[type=submit],input[type=submit]') || f.querySelector('button:not([type])'));
        submit = b ? kind(b) + ' ' + label(b) : '';
      }
      return answer({ kind: kind(e), label: label(e), submit: submit });
    }

    if (A.op === 'click') {
      const e = find();
      if (!e) return answer({ gone: true });
      e.scrollIntoView({ block: 'center', inline: 'center' });
      const r = e.getBoundingClientRect();
      const at = { bubbles: true, cancelable: true, composed: true, view: window, clientX: r.left + r.width / 2, clientY: r.top + r.height / 2 };
      const fire = (type, C, extra) => e.dispatchEvent(new C(type, Object.assign({}, at, extra || {})));
      const P = window.PointerEvent || MouseEvent;
      if (A.how === 'hover') {
        fire('pointerover', P);
        fire('pointerenter', P, { bubbles: false });
        fire('mouseover', MouseEvent);
        fire('mouseenter', MouseEvent, { bubbles: false });
        fire('mousemove', MouseEvent);
      } else if (A.how === 'right') {
        fire('pointerdown', P, { button: 2, buttons: 2 });
        fire('mousedown', MouseEvent, { button: 2, buttons: 2 });
        fire('pointerup', P, { button: 2 });
        fire('mouseup', MouseEvent, { button: 2 });
        fire('contextmenu', MouseEvent, { button: 2 });
      } else {
        if (e.focus) e.focus({ preventScroll: true });
        const once = (n) => {
          fire('pointerdown', P, { buttons: 1, detail: n });
          fire('mousedown', MouseEvent, { buttons: 1, detail: n });
          fire('pointerup', P, { detail: n });
          fire('mouseup', MouseEvent, { detail: n });
          e.click();
        };
        once(1);
        if (A.how === 'double') {
          once(2);
          fire('dblclick', MouseEvent, { detail: 2 });
        }
      }
      return answer({ ok: true, url: location.href });
    }

    if (A.op === 'fill') {
      const e = find();
      if (!e) return answer({ gone: true });
      if (secret(e)) return answer({ secret: true });
      const fire = (t) => e.dispatchEvent(new Event(t, { bubbles: true }));
      e.scrollIntoView({ block: 'center' });
      if (e.focus) e.focus({ preventScroll: true });
      let chose = null;
      if (e.tagName === 'SELECT') {
        const want = clean(A.text).toLowerCase();
        const opts = Array.from(e.options);
        const o = opts.find((o) => clean(o.text).toLowerCase() === want) || opts.find((o) => o.value.toLowerCase() === want) || opts.find((o) => clean(o.text).toLowerCase().includes(want));
        if (!o) return answer({ options: opts.slice(0, 40).map((o) => clean(o.text)) });
        e.value = o.value;
        chose = clean(o.text);
        fire('input');
        fire('change');
      } else if ((e.tagName === 'INPUT' && kind(e) !== 'file picker' && kind(e) !== 'checkbox' && kind(e) !== 'radio' && kind(e) !== 'button') || e.tagName === 'TEXTAREA') {
        const proto = e.tagName === 'INPUT' ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
        Object.getOwnPropertyDescriptor(proto, 'value').set.call(e, A.text);
        fire('input');
        fire('change');
      } else if (e.isContentEditable) {
        const range = document.createRange();
        range.selectNodeContents(e);
        const sel = getSelection();
        sel.removeAllRanges();
        sel.addRange(range);
        if (!document.execCommand('insertText', false, A.text)) {
          e.innerText = A.text;
          fire('input');
        }
      } else {
        return answer({ field: false });
      }
      if (A.submit) {
        const f = e.form || e.closest('form');
        if (f && f.requestSubmit) {
          f.requestSubmit();
        } else {
          for (const t of ['keydown', 'keypress', 'keyup']) {
            e.dispatchEvent(new KeyboardEvent(t, { key: 'Enter', code: 'Enter', keyCode: 13, which: 13, bubbles: true, cancelable: true }));
          }
        }
      }
      return answer({ ok: true, chose: chose });
    }

    if (A.op === 'probe') {
      const want = A.text ? clean(A.text).toLowerCase() : '';
      const body = document.body ? clean(document.body.innerText).toLowerCase() : '';
      return answer({ ready: document.readyState === 'complete', found: !want || body.includes(want) });
    }

    if (A.op === 'context') {
      const a = document.activeElement;
      const inSecret = a && secret(a);
      return answer({ url: location.href, text: inSecret ? '' : String(getSelection() || '').slice(0, 4000) });
    }

    return answer({ error: 'unknown page operation' });
  } catch (err) {
    return JSON.stringify({ error: 'the page script failed: ' + String(err && err.message || err).slice(0, 200) });
  }
})

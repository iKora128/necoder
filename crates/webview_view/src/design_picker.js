// necoder Design Mode の要素ピッカー。Web タブ（localhost）の最上位の文書にだけ、文書の頭で入れておく。
// 普段は何もしない — necoder が `window.__necoderDesign.start(nonce)` を呼んだ間だけ、ホバーで枠と
// ラベルを出し、クリックで要素の情報を `window.ipc.postMessage` で necoder へ渡す。
//
// 出典: stablyai/orca@646e9a5 の src/main/browser/grab-guest-foundation-script.ts /
// grab-guest-content-script.ts / grab-guest-element-context-script.ts / grab-guest-react-script.ts /
// grab-guest-overlay-script.ts（MIT）。要素情報の抜き出し方（予算で切る・安定したセレクタ・近くの
// テキスト・計算済みスタイル・React の fiber）とホバー枠の作り方を参考に、necoder の IPC・nonce・
// 伏せ字の規則へ合わせて書き直した。React 19 の `_debugStack` と `data-inspector-*` は necoder で足した。
//
// 受け側（Rust の `webview_view::design`）も大きさと形を検証する。ここの予算はそれより小さく切る。
(function () {
  'use strict';
  if (window.__necoderDesign) {
    return;
  }
  // ページの script より先に掴んでおく（後から差し替えられても使わない）。
  var stringify = JSON.stringify;
  var BUDGET = {
    html: 8000,
    text: 2000,
    nearbyEntry: 200,
    nearbyCount: 6,
    selector: 700,
    path: 700,
    title: 300,
    attributeValue: 300,
    attributeCount: 16,
    components: 8,
    componentName: 80,
    source: 400,
    accessibleName: 200
  };
  var SAFE_ATTRIBUTES = [
    'id', 'class', 'name', 'type', 'role', 'href', 'src', 'alt', 'title', 'placeholder', 'for',
    'action', 'method'
  ];
  var URL_ATTRIBUTES = ['href', 'src', 'action', 'srcset', 'poster', 'data'];
  // 伏せる対象: 名前がそれらしい属性（`data-api-key` など）の値と、値の中の「鍵=値」「Bearer …」・JWT。
  // `type="password"` や `class="password-field"` のような語そのものは伏せない（情報を失うだけ）。
  var SECRET_NAME = /(access_token|auth_token|api[_-]?key|apikey|client_secret|oauth_state|session[_-]?id|csrf|secret|passwd|password|token|x-amz-)/i;
  var SECRET_VALUE = /((access_token|auth_token|api[_-]?key|apikey|client_secret|oauth_state|session[_-]?id|csrf[_-]?token|csrf|secret|password|passwd|token)\s*[=:])|(bearer\s+\S)|(x-amz-)|(eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,})/i;
  var STYLE_PROPERTIES = [
    'display', 'position', 'width', 'height', 'margin', 'padding', 'color', 'background-color',
    'border', 'border-radius', 'font-family', 'font-size', 'font-weight', 'line-height',
    'text-align', 'z-index'
  ];
  var REDACTED = '[redacted]';

  var state = null;

  function clamp(value, max) {
    var text = value == null ? '' : String(value);
    return text.length <= max ? text : text.slice(0, max - 1) + '…';
  }

  function secretName(name) {
    return SECRET_NAME.test(String(name || ''));
  }

  function hasSecret(value) {
    return SECRET_VALUE.test(String(value || ''));
  }

  // query と fragment（トークンが乗りやすい）を落とす。http(s) 以外は出さない。
  function sanitizeUrl(value) {
    try {
      var url = new URL(value, location.href);
      if (url.protocol !== 'http:' && url.protocol !== 'https:') {
        return '';
      }
      url.search = '';
      url.hash = '';
      url.username = '';
      url.password = '';
      return url.toString();
    } catch (error) {
      return '';
    }
  }

  // 属性値の URL は相対のまま query と fragment だけ落とす（HTML の見た目を変えない）。
  function stripUrlSuffix(value) {
    return String(value || '').split(/[?#]/)[0];
  }

  function collapse(text) {
    return String(text || '').replace(/\s+/g, ' ').trim();
  }

  function visibleText(element, max) {
    var text = '';
    try {
      text = element.innerText != null ? element.innerText : element.textContent;
    } catch (error) {
      text = element.textContent;
    }
    return clamp(collapse(text), max);
  }

  function cssEscape(value) {
    if (window.CSS && typeof window.CSS.escape === 'function') {
      return window.CSS.escape(value);
    }
    return String(value).replace(/[^a-zA-Z0-9_-]/g, function (character) {
      return '\\' + character;
    });
  }

  // ビルドが付けたハッシュ風のクラス（`css-1x2y3z`・`sc-AbCdEf123`）はセレクタに使わない。
  function looksGenerated(name) {
    return /^css-[a-z0-9]+$/i.test(name) ||
      (/^[A-Za-z0-9_-]{10,}$/.test(name) && /\d/.test(name) && /[A-Z]/.test(name));
  }

  function stableClasses(element, count) {
    var result = [];
    if (!element.classList) {
      return result;
    }
    for (var index = 0; index < element.classList.length && result.length < count; index++) {
      var name = element.classList[index];
      if (!name || name.length > 60 || hasSecret(name) || looksGenerated(name)) {
        continue;
      }
      result.push(name);
    }
    return result;
  }

  function selectorPart(element) {
    var tag = element.tagName.toLowerCase();
    if (element.id && !hasSecret(element.id)) {
      return tag + '#' + cssEscape(element.id);
    }
    var classes = stableClasses(element, 2);
    return tag + classes.map(function (name) { return '.' + cssEscape(name); }).join('');
  }

  function isUnique(selector) {
    try {
      return document.querySelectorAll(selector).length === 1;
    } catch (error) {
      return false;
    }
  }

  function nthOfType(element) {
    var index = 1;
    var sibling = element.previousElementSibling;
    while (sibling) {
      if (sibling.tagName === element.tagName) {
        index++;
      }
      sibling = sibling.previousElementSibling;
    }
    return index > 1 ? ':nth-of-type(' + index + ')' : '';
  }

  // 一意になるまで祖先をさかのぼる短いセレクタ。
  function uniqueSelector(element) {
    var parts = [];
    var current = element;
    while (current && current.nodeType === 1 && current !== document.body && parts.length < 8) {
      var part = selectorPart(current);
      var candidate = [part].concat(parts).join(' > ');
      if (!isUnique(candidate) && current.parentElement) {
        part += nthOfType(current);
      }
      parts.unshift(part);
      var selector = parts.join(' > ');
      if (isUnique(selector)) {
        return clamp(selector, BUDGET.selector);
      }
      current = current.parentElement;
    }
    return clamp(parts.join(' > ') || element.tagName.toLowerCase(), BUDGET.selector);
  }

  // 人が読む祖先の道筋（`main.card > form > button#start`）。
  function readablePath(element) {
    var parts = [];
    var current = element;
    while (current && current.nodeType === 1 && parts.length < 6) {
      var tag = current.tagName.toLowerCase();
      if (tag === 'html' || tag === 'body') {
        break;
      }
      parts.unshift(selectorPart(current));
      current = current.parentElement;
    }
    return clamp(parts.join(' > '), BUDGET.path);
  }

  function nearbyText(element) {
    var result = [];
    var parent = element.parentElement;
    if (!parent) {
      return result;
    }
    var siblings = parent.children;
    for (var index = 0; index < siblings.length && result.length < BUDGET.nearbyCount; index++) {
      var sibling = siblings[index];
      if (sibling === element) {
        continue;
      }
      var text = visibleText(sibling, BUDGET.nearbyEntry);
      if (text) {
        result.push(text);
      }
    }
    return result;
  }

  function computedStyles(element) {
    var styles = window.getComputedStyle(element);
    var result = {};
    for (var index = 0; index < STYLE_PROPERTIES.length; index++) {
      var name = STYLE_PROPERTIES[index];
      result[name] = clamp(styles.getPropertyValue(name), 200);
    }
    return result;
  }

  // HTML の抜粋: script を落とし、password の値と secret らしい属性値を伏せ、URL の query を落とす。
  function safeHtml(element) {
    var clone = element.cloneNode(true);
    var nodes = [clone];
    var descendants = clone.querySelectorAll ? clone.querySelectorAll('*') : [];
    for (var index = 0; index < descendants.length; index++) {
      nodes.push(descendants[index]);
    }
    for (var nodeIndex = 0; nodeIndex < nodes.length; nodeIndex++) {
      var node = nodes[nodeIndex];
      if (node.tagName === 'SCRIPT') {
        if (node.parentNode) {
          node.parentNode.removeChild(node);
        }
        continue;
      }
      var isPassword = node.tagName === 'INPUT' &&
        String(node.getAttribute('type') || '').toLowerCase() === 'password';
      var attributes = Array.prototype.slice.call(node.attributes || []);
      for (var attributeIndex = 0; attributeIndex < attributes.length; attributeIndex++) {
        var attribute = attributes[attributeIndex];
        var name = attribute.name.toLowerCase();
        // URL は query を落としてから見る（トークンは大抵 query に乗る。道筋まで伏せない）。
        var value = URL_ATTRIBUTES.indexOf(name) !== -1 ? stripUrlSuffix(attribute.value) : attribute.value;
        if ((isPassword && name === 'value') || secretName(name) || hasSecret(value)) {
          node.setAttribute(attribute.name, REDACTED);
        } else if (value !== attribute.value) {
          node.setAttribute(attribute.name, value);
        }
      }
    }
    return clamp(clone.outerHTML || '', BUDGET.html);
  }

  function safeAttributes(element) {
    var result = [];
    var attributes = element.attributes || [];
    for (var index = 0; index < attributes.length && result.length < BUDGET.attributeCount; index++) {
      var name = attributes[index].name.toLowerCase();
      if (SAFE_ATTRIBUTES.indexOf(name) === -1 && name.indexOf('aria-') !== 0) {
        continue;
      }
      var value = attributes[index].value;
      if (URL_ATTRIBUTES.indexOf(name) !== -1) {
        value = stripUrlSuffix(value);
      }
      if (secretName(name) || hasSecret(value)) {
        value = REDACTED;
      }
      result.push({ name: name, value: clamp(value, BUDGET.attributeValue) });
    }
    return result;
  }

  function accessibleName(element) {
    var label = element.getAttribute('aria-label');
    if (label) {
      return clamp(collapse(label), BUDGET.accessibleName);
    }
    var labelledBy = element.getAttribute('aria-labelledby');
    if (labelledBy) {
      var names = labelledBy.split(/\s+/).slice(0, 8).map(function (id) {
        var target = document.getElementById(id);
        return target ? visibleText(target, 100) : '';
      }).filter(Boolean);
      if (names.length) {
        return clamp(names.join(' '), BUDGET.accessibleName);
      }
    }
    var tag = element.tagName.toLowerCase();
    if (tag === 'button' || tag === 'a' || tag === 'label' || tag === 'summary') {
      var text = visibleText(element, BUDGET.accessibleName);
      if (text) {
        return text;
      }
    }
    var fallback = element.getAttribute('title') || element.getAttribute('alt') ||
      element.getAttribute('placeholder');
    return fallback ? clamp(collapse(fallback), BUDGET.accessibleName) : null;
  }

  // ── ソースの位置（開発ビルド）──

  function cleanSourcePath(path) {
    return String(path || '')
      .replace(/[?#].*$/, '')
      .replace(/^webpack-internal:\/\/\/(\.\/)?/, '')
      .replace(/^webpack:\/\/\/(\.\/)?/, '')
      .replace(/^turbopack:\/\/\/(\[project\]\/)?/, '')
      .replace(/^https?:\/\/[^/]+\//, '')
      .replace(/^file:\/\//, '')
      .replace(/^\/@fs\//, '/')
      .replace(/^\.\//, '');
  }

  function sourceFromFields(file, line, column, via) {
    var path = cleanSourcePath(file);
    var lineNumber = parseInt(line, 10);
    if (!path || !(lineNumber > 0) || hasSecret(path)) {
      return null;
    }
    var columnNumber = parseInt(column, 10);
    return {
      file: clamp(path, BUDGET.source),
      line: lineNumber,
      column: columnNumber > 0 ? columnNumber : null,
      via: via
    };
  }

  // `data-inspector-relative-path` / `-line` / `-column`（react-dev-inspector など）か
  // `data-source="file:line:column"` を持つ一番近い祖先。
  function dataAttributeSource(element) {
    var current = element;
    for (var depth = 0; current && current.nodeType === 1 && depth < 12; depth++) {
      var path = current.getAttribute('data-inspector-relative-path');
      if (path) {
        return sourceFromFields(path, current.getAttribute('data-inspector-line'),
          current.getAttribute('data-inspector-column'), 'data-inspector');
      }
      var compact = current.getAttribute('data-source');
      var match = compact && /^(.*?):(\d+)(?::(\d+))?$/.exec(compact);
      if (match) {
        return sourceFromFields(match[1], match[2], match[3], 'data-source');
      }
      current = current.parentElement;
    }
    return null;
  }

  function fiberOf(element) {
    var keys = Object.keys(element);
    for (var index = 0; index < keys.length; index++) {
      if (keys[index].indexOf('__reactFiber$') === 0 ||
        keys[index].indexOf('__reactInternalInstance$') === 0) {
        try {
          return element[keys[index]] || null;
        } catch (error) {
          return null;
        }
      }
    }
    return null;
  }

  function componentName(fiber) {
    var type = fiber && (fiber.type || fiber.elementType);
    if (!type || typeof type === 'string') {
      return null;
    }
    var name = type.displayName || type.name ||
      (type.render && (type.render.displayName || type.render.name)) ||
      (type.type && (type.type.displayName || type.type.name));
    if (!name || name.length <= 1 ||
      /^(Fragment|Root|Routes|Route|Outlet|Provider|Consumer|Profiler|Suspense|StrictMode)$/.test(name) ||
      /(Boundary|Provider|Consumer|Context|Router)$/.test(name)) {
      return null;
    }
    return clamp(name, BUDGET.componentName);
  }

  // React 19 は `_debugSource` を廃止し、JSX を作った場所のスタックを `_debugStack`（Error）に残す。
  // React 自身と node_modules のフレームを飛ばした最初のフレームを「その要素を書いた場所」とみなす。
  function sourceFromDebugStack(stack) {
    var text = stack && (stack.stack || String(stack));
    if (!text) {
      return null;
    }
    var lines = String(text).split('\n');
    for (var index = 0; index < lines.length; index++) {
      var line = lines[index];
      if (/node_modules|react-dom|react\.development|jsx-dev-runtime|jsx-runtime|react-stack-top-frame/.test(line)) {
        continue;
      }
      var match = /\(?((?:https?|webpack-internal|webpack|file|turbopack):\/\/[^\s)]+?):(\d+):(\d+)\)?\s*$/.exec(line);
      if (match) {
        return sourceFromFields(match[1], match[2], match[3], 'debugStack');
      }
    }
    return null;
  }

  function reactInfo(element) {
    var components = [];
    var source = null;
    try {
      var fiber = fiberOf(element);
      for (var depth = 0; fiber && depth < 40; depth++) {
        var name = componentName(fiber);
        if (name && components.indexOf(name) === -1 && components.length < BUDGET.components) {
          components.push(name);
        }
        if (!source) {
          // React 18 以前: JSX の場所（babel の jsx-source）。
          var debugSource = fiber._debugSource ||
            (fiber._debugOwner && fiber._debugOwner._debugSource);
          if (debugSource && debugSource.fileName) {
            source = sourceFromFields(debugSource.fileName, debugSource.lineNumber,
              debugSource.columnNumber, 'debugSource');
          } else if (fiber._debugStack) {
            source = sourceFromDebugStack(fiber._debugStack);
          }
        }
        fiber = fiber.return;
      }
    } catch (error) {
      // 開発ビルドでない・形が違う: 無しでよい。
    }
    return { components: components.reverse(), source: source };
  }

  function capture(element) {
    var rect = element.getBoundingClientRect();
    var react = reactInfo(element);
    var role = element.getAttribute('role');
    return {
      page: {
        url: sanitizeUrl(location.href),
        title: clamp(document.title, BUDGET.title),
        viewport_width: window.innerWidth,
        viewport_height: window.innerHeight,
        device_pixel_ratio: window.devicePixelRatio || 1
      },
      element: {
        tag: element.tagName.toLowerCase(),
        selector: uniqueSelector(element),
        path: readablePath(element),
        text: visibleText(element, BUDGET.text),
        nearby_text: nearbyText(element),
        html: safeHtml(element),
        role: role ? clamp(role, 80) : null,
        accessible_name: accessibleName(element),
        attributes: safeAttributes(element),
        rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
        styles: computedStyles(element),
        components: react.components,
        source: dataAttributeSource(element) || react.source
      }
    };
  }

  function send(message) {
    try {
      window.ipc.postMessage(stringify(message));
    } catch (error) {
      // IPC が無い（Web タブ以外）: 何もしない。
    }
  }

  // ── ホバー枠（ページの上に載せる閉じた shadow root・色は中立）──

  function hoverLabel(element, rect) {
    var parts = [selectorPart(element)];
    var react = reactInfo(element);
    if (react.components.length) {
      parts.unshift('<' + react.components[react.components.length - 1] + '>');
    }
    var text = visibleText(element, 30);
    if (text) {
      parts.push('"' + text + '"');
    }
    parts.push(Math.round(rect.width) + '×' + Math.round(rect.height));
    return parts.join('  ');
  }

  function highlight(element) {
    if (!state) {
      return;
    }
    if (!element || element === document.documentElement || element === document.body ||
      element === state.host) {
      state.box.style.display = 'none';
      state.label.style.display = 'none';
      state.current = null;
      return;
    }
    state.current = element;
    var rect = element.getBoundingClientRect();
    state.box.style.left = rect.x + 'px';
    state.box.style.top = rect.y + 'px';
    state.box.style.width = rect.width + 'px';
    state.box.style.height = rect.height + 'px';
    state.box.style.display = 'block';
    state.label.textContent = hoverLabel(element, rect);
    var top = rect.bottom + 6;
    if (top + 26 > window.innerHeight) {
      top = Math.max(4, rect.top - 28);
    }
    state.label.style.left = Math.max(4, Math.min(rect.x, window.innerWidth - 320)) + 'px';
    state.label.style.top = top + 'px';
    state.label.style.display = 'block';
  }

  function elementAt(x, y) {
    state.host.style.pointerEvents = 'none';
    var element = document.elementFromPoint(x, y);
    state.host.style.pointerEvents = 'auto';
    return element;
  }

  // 1 つ確定する。切り抜きに枠が写らないよう、枠を消して 2 フレーム待ってから渡す。
  // ページが見えていない間は requestAnimationFrame が止まるので、時計でも 1 度だけ渡す（先に来た方）。
  // 次の選択は necoder が切り抜きを終えて `resume()` を呼ぶまで受けない。
  function pick(element, multi) {
    if (!state || state.paused || !element) {
      return;
    }
    state.paused = true;
    var message = { type: 'pick', nonce: state.nonce, multi: !!multi, capture: capture(element) };
    var delivered = false;
    function deliver() {
      if (!delivered) {
        delivered = true;
        send(message);
      }
    }
    state.host.style.display = 'none';
    window.requestAnimationFrame(function () {
      window.requestAnimationFrame(deliver);
    });
    window.setTimeout(deliver, 150);
  }

  function onMouseMove(event) {
    if (!state || state.paused) {
      return;
    }
    highlight(elementAt(event.clientX, event.clientY));
  }

  function swallow(event) {
    event.preventDefault();
    event.stopPropagation();
    event.stopImmediatePropagation();
  }

  function onClick(event) {
    swallow(event);
    // ページの script が合成したクリック（isTrusted でない）では選ばない。
    if (!event.isTrusted || !state) {
      return;
    }
    var element = state.current || elementAt(event.clientX, event.clientY);
    pick(element, event.shiftKey);
  }

  function onKeyDown(event) {
    if (!state || event.key !== 'Escape' || !event.isTrusted) {
      return;
    }
    swallow(event);
    send({ type: 'cancel', nonce: state.nonce });
    stop();
  }

  function start(nonce) {
    stop();
    if (typeof nonce !== 'string' || !/^[0-9a-f]{16,64}$/.test(nonce)) {
      return false;
    }
    var host = document.createElement('div');
    host.setAttribute('data-necoder-design', '');
    host.style.cssText = 'position:fixed;inset:0;z-index:2147483647;cursor:crosshair;pointer-events:auto;background:transparent;';
    var shadow = host.attachShadow({ mode: 'closed' });
    var box = document.createElement('div');
    box.style.cssText = 'position:fixed;display:none;pointer-events:none;box-sizing:border-box;' +
      'border:2px solid rgba(255,255,255,0.92);border-radius:3px;background:rgba(255,255,255,0.06);' +
      'box-shadow:0 0 0 1px rgba(0,0,0,0.45),0 2px 10px rgba(0,0,0,0.25);';
    var label = document.createElement('div');
    label.style.cssText = 'position:fixed;display:none;pointer-events:none;max-width:320px;overflow:hidden;' +
      'white-space:nowrap;text-overflow:ellipsis;padding:3px 8px;border-radius:4px;' +
      'background:rgba(24,26,32,0.94);color:#e8e8ea;font:11px/1.4 -apple-system,BlinkMacSystemFont,' +
      '"Segoe UI",sans-serif;box-shadow:0 2px 8px rgba(0,0,0,0.3);';
    shadow.appendChild(box);
    shadow.appendChild(label);
    (document.documentElement || document.body).appendChild(host);
    host.addEventListener('mousemove', onMouseMove, true);
    host.addEventListener('click', onClick, true);
    host.addEventListener('mousedown', swallow, true);
    host.addEventListener('mouseup', swallow, true);
    host.addEventListener('contextmenu', swallow, true);
    window.addEventListener('keydown', onKeyDown, true);
    state = { nonce: nonce, host: host, box: box, label: label, current: null, paused: false };
    return true;
  }

  function stop() {
    if (!state) {
      return true;
    }
    window.removeEventListener('keydown', onKeyDown, true);
    if (state.host.parentNode) {
      state.host.parentNode.removeChild(state.host);
    }
    state = null;
    return true;
  }

  // 切り抜きが済んだ（⇧ で続けて選ぶ時）。枠を戻して次を受ける。
  function resume() {
    if (!state) {
      return false;
    }
    state.paused = false;
    state.host.style.display = '';
    highlight(state.current);
    return true;
  }

  /* necoder:debug-hooks */

  Object.defineProperty(window, '__necoderDesign', {
    value: Object.freeze({ start: start, stop: stop, resume: resume }),
    writable: false,
    configurable: false,
    enumerable: false
  });
})();

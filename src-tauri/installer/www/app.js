/* app.js — AniBox 安装器页面状态机 + JS桥 */
(function () {
  'use strict';
  var $ = function (id) { return document.getElementById(id); };
  var host = (window.chrome && window.chrome.webview) ? window.chrome.webview : null;

  function send(m) { if (host) host.postMessage(JSON.stringify(m)); }

  /* ── Toast ── */
  var toastTimer = null;
  function toast(msg) {
    var t = $('toast');
    t.textContent = msg; t.hidden = false;
    requestAnimationFrame(function () { t.classList.add('show'); });
    clearTimeout(toastTimer);
    toastTimer = setTimeout(function () {
      t.classList.remove('show');
      setTimeout(function () { t.hidden = true; }, 320);
    }, 2400);
  }

  /* ── 屏幕切换 ── */
  var screens = ['welcome', 'options', 'progress', 'finish', 'unconfirm', 'unfinish', 'error'];
  var badgeTexts = { welcome: 'SETUP', options: 'SETUP', progress: 'SETUP', finish: 'DONE',
    unconfirm: 'UNINSTALL', unfinish: 'DONE', error: 'ERROR' };
  function show(name) {
    screens.forEach(function (s) {
      var el = $('screen-' + s);
      if (s === name) el.classList.add('active'); else el.classList.remove('active');
    });
    var badge = badgeTexts[name] || 'SETUP';
    if (state.uninstallMode && name === 'progress') badge = 'UNINSTALL';
    $('chromeBadge').textContent = badge;
  }

  /* ── 状态 ── */
  var state = { agreed: false, mode: 'user', path: '', exeName: 'anibox.exe', version: '', installing: false,
    uninstallMode: false, uninstalling: false, uninstalled: false, purge: false,
    installSize: 0, dataSize: 0, dataPath: '' };

  function setMode(mode) {
    state.mode = mode;
    $('cardUser').classList.toggle('selected', mode === 'user');
    $('cardAll').classList.toggle('selected', mode === 'all');
    $('cardUser').querySelector('.radio').className = 'radio' + (mode === 'user' ? ' on' : '');
    $('cardAll').querySelector('.radio').className = 'radio' + (mode === 'all' ? ' on' : '');
    var needAdmin = mode === 'all';
    $('modeHint').textContent = needAdmin
      ? '将为所有用户安装（通常位于 Program Files）, 点击「开始安装」后会弹出管理员授权。'
      : '将为当前用户安装, 无需管理员权限。';
    send({ type: 'modeChanged', mode: mode });
  }
  function setPath(p) {
    state.path = p;
    $('pathText').textContent = p || '';
    $('pathText').title = p || '';
  }

  /* ── ① 欢迎页 ── */
  $('agreeRow').addEventListener('click', function (e) {
    if (e.target.tagName === 'A') return;
    state.agreed = !state.agreed;
    $('agreeRow').classList.toggle('on', state.agreed);
    $('btnWelcomeNext').disabled = !state.agreed;
  });
  $('btnWelcomeNext').addEventListener('click', function () {
    if (!state.agreed) return;
    show('options');
  });

  /* ── 许可协议 ── */
  var licLoaded = false;
  function openLicense() {
    var ov = $('licOverlay');
    ov.hidden = false;
    requestAnimationFrame(function () { ov.classList.add('show'); });
    if (!licLoaded) {
      fetch('license.html', { cache: 'no-store' })
        .then(function (r) { return r.ok ? r.text() : Promise.reject(0); })
        .then(function (t) { $('licBody').innerHTML = t; licLoaded = true; })
        .catch(function () { $('licBody').textContent = '许可协议文件缺失。本软件按仓库所附开源协议发布。'; });
    }
  }
  function closeLicense(accept) {
    var ov = $('licOverlay');
    ov.classList.remove('show');
    setTimeout(function () { ov.hidden = true; }, 260);
    if (accept) {
      if (!state.agreed) { state.agreed = true; $('agreeRow').classList.add('on'); $('btnWelcomeNext').disabled = false; }
    }
  }
  $('linkLicense').addEventListener('click', function (e) { e.preventDefault(); openLicense(); });
  $('licClose').addEventListener('click', function () { closeLicense(false); });
  $('licAccept').addEventListener('click', function () { closeLicense(true); });

  /* ── ② 选项页 ── */
  $('cardUser').addEventListener('click', function () { setMode('user'); });
  $('cardAll').addEventListener('click', function () { setMode('all'); });
  $('btnBrowse').addEventListener('click', function () { send({ type: 'pickFolder' }); });
  $('btnBack').addEventListener('click', function () { show('welcome'); });
  $('btnInstall').addEventListener('click', function () {
    if (state.installing) return;
    if (!state.path) { toast('请先选择安装位置'); return; }
    state.installing = true;
    show('progress');
    setProgress(0, 'prepare', '正在准备安装…');
    send({ type: 'install', mode: state.mode, path: state.path });
  });

  /* ── ③ 进度页 ── */
  var shownPct = -1;
  var phaseTexts = { prepare: '正在准备安装…', extract: '正在解压应用文件…', settle: '正在写入注册表与快捷方式…' };
  var unPhaseTexts = { prepare: '正在准备卸载…', delete: '正在删除应用文件…', purge: '正在删除用户数据…',
    settle: '正在清理注册表与快捷方式…', done: '卸载完成' };
  function setProgress(pct, phase, file) {
    var p = Math.max(0, Math.min(100, pct));
    if (p !== shownPct) {
      shownPct = p;
      $('pctNum').textContent = Math.round(p);
      $('barFill').style.width = p + '%';
      var C = 2 * Math.PI * 52;
      $('ringFill').style.strokeDashoffset = (C * (1 - p / 100)).toFixed(1);
    }
    var texts = state.uninstallMode ? unPhaseTexts : phaseTexts;
    $('progFile').textContent = file || texts[phase] || '';
    if (phase === 'settle') $('progSub').textContent = state.uninstallMode
      ? '正在收尾 · 请稍候' : '即将完成 · 请稍候';
    if (phase === 'purge') $('progSub').textContent = '正在删除用户数据 · 此操作不可恢复';
  }

  /* ── 卸载页 ── */
  function fmtSize(n) {
    n = Number(n) || 0;
    if (n <= 0) return '0 MB';
    var mb = n / 1048576;
    if (mb >= 1024) return (mb / 1024).toFixed(2) + ' GB';
    return (mb >= 100 ? mb.toFixed(0) : mb.toFixed(1)) + ' MB';
  }
  function setPurge(on) {
    state.purge = !!on;
    $('purgeRow').classList.toggle('on', state.purge);
  }
  function renderUninstallMeta(m) {
    state.uninstallMode = true;
    state.version = m.version || '';
    state.path = m.path || '';
    state.installSize = Number(m.installSize) || 0;
    state.dataSize = Number(m.dataSize) || 0;
    state.dataPath = m.dataPath || '';
    state.exeName = 'anibox.exe';
    $('unVersion').textContent = m.version ? 'v' + m.version : '未知';
    $('unPath').textContent = m.path || '未检测到安装目录';
    $('unPath').title = m.path || '';
    $('unSize').textContent = fmtSize(state.installSize);
    $('dataPath').textContent = state.dataPath || '%APPDATA%\\com.anibox.app';
    $('dataSize').textContent = state.dataSize > 0 ? '当前占用 ' + fmtSize(state.dataSize) : '当前没有用户数据';
    $('confirmPaths').textContent = state.dataPath || '';
    $('floorTag').textContent = 'ANIBOX · V' + state.version;
    document.title = 'AniBox 卸载';
    setPurge(String(m.purgeDefault) === '1');
    show('unconfirm');
  }
  function startUninstall(purge) {
    if (state.uninstalling) return;
    state.uninstalling = true;
    setPurge(purge);
    closePurgeConfirm();
    $('progTitle').textContent = '正在卸载';
    $('progSub').textContent = '正在移除 AniBox 及其组件 · 请稍候';
    shownPct = -1;
    setProgress(0, 'prepare', '');
    show('progress');
    send({ type: 'uninstall', purge: purge ? '1' : '0' });
  }
  function openPurgeConfirm() {
    var ov = $('purgeOverlay');
    ov.hidden = false;
    requestAnimationFrame(function () { ov.classList.add('show'); });
  }
  function closePurgeConfirm() {
    var ov = $('purgeOverlay');
    ov.classList.remove('show');
    setTimeout(function () { ov.hidden = true; }, 260);
  }
  $('purgeRow').addEventListener('click', function () { setPurge(!state.purge); });
  $('btnUnCancel').addEventListener('click', function () { send({ type: 'close' }); });
  $('btnUninstall').addEventListener('click', function () {
    if (state.uninstalling) return;
    if (state.purge) openPurgeConfirm();
    else startUninstall(false);
  });
  $('btnPurgeCancel').addEventListener('click', function () { closePurgeConfirm(); });
  $('btnPurgeConfirm').addEventListener('click', function () { startUninstall(true); });
  $('btnReinstall').addEventListener('click', function () { send({ type: 'reinstall' }); });
  $('btnUnClose').addEventListener('click', function () { send({ type: 'close' }); });
  $('btnErrClose').addEventListener('click', function () { send({ type: 'close' }); });
  $('btnErrBack').addEventListener('click', function () {
    state.uninstalling = false;
    show('unconfirm');
  });

  /* ── ④ 完成页 ── */
  $('btnLaunch').addEventListener('click', function () { send({ type: 'launch' }); });
  $('btnFinishClose').addEventListener('click', function () { send({ type: 'close' }); });

  /* ── chrome ── */
  $('btnMin').addEventListener('click', function () { send({ type: 'minimize' }); });
  $('btnClose').addEventListener('click', function () {
    if (state.installing && !$('screen-finish').classList.contains('active')) {
      toast('正在安装中, 请等待完成');
      return;
    }
    if (state.uninstalling) {
      toast('正在卸载中, 请等待完成');
      return;
    }
    send({ type: 'close' });
  });
  document.querySelectorAll('[data-drag]').forEach(function (el) {
    el.addEventListener('mousedown', function (e) {
      // 只做窗口拖动; 点在可交互控件上时放行, 否则 DragMove 会吞掉本次点击
      if (e.button !== 0) return;
      if (e.target && e.target.closest && e.target.closest('button, a, input, .agree, .purgerow, .opt-left .optcard')) return;
      send({ type: 'drag' });
    });
  });
  document.addEventListener('keydown', function (e) {
    if (e.key === 'Enter') {
      if (!$('purgeOverlay').hidden) { $('btnPurgeConfirm').click(); return; }
      if ($('screen-welcome').classList.contains('active') && state.agreed) show('options');
      else if ($('screen-options').classList.contains('active')) $('btnInstall').click();
      else if ($('screen-unconfirm').classList.contains('active')) $('btnUninstall').click();
    }
    if (e.key === 'Escape') {
      if (!$('purgeOverlay').hidden) closePurgeConfirm();
      else if (!$('licOverlay').hidden) closeLicense(false);
    }
  });

  /* ── 安装说明状态条: 首次安装 / 版本覆盖升级 / 发布日期 ── */
  function renderInstallInfo(m) {
    var el = $('instInfo');
    var txt = $('instInfoText');
    var date = m.releaseDate || '';
    var legacyCount = parseInt(m.legacyCount || '0', 10) || 0;
    var sameVersion = m.sameVersion === '1' || m.sameVersion === 1 || m.sameVersion === true;
    if (sameVersion && m.installed) {
      // 本机已装同一版本：显示「覆盖重装」，即便还有旧版残留也不能说成「升级安装」。
      el.classList.add('upgrade');
      txt.innerHTML = '<b>覆盖重装</b> · 已安装 v' + m.installed + ' · 将覆盖修复（配置保留）';
      if (legacyCount > 0) {
        var note2 = document.createElement('span');
        note2.className = 'ii-legacy';
        note2.textContent = '另检测到旧版残留：' + (m.legacyVersions || '') + '，安装时将一并清理';
        txt.appendChild(note2);
      }
    } else if (legacyCount > 0) {
      el.classList.add('upgrade');
      var from = m.installed ? ('v' + m.installed) : '旧版本';
      txt.innerHTML = '<b>升级安装</b> · ' + from + ' → v' + m.version +
        '（旧版本将被替换，用户数据会保留并自动迁移）';
      if (legacyCount > 1) {
        var note = document.createElement('span');
        note.className = 'ii-legacy';
        note.textContent = '检测到 ' + legacyCount + ' 个旧版本：' + (m.legacyVersions || '') +
          '，安装时将一并清理';
        txt.appendChild(note);
      }
    } else if (m.installed) {
      el.classList.add('upgrade');
      if (m.installed === m.version) {
        txt.innerHTML = '<b>覆盖重装</b> · 已安装 v' + m.installed + ' · 将覆盖修复（配置保留）';
      } else {
        txt.innerHTML = '<b>覆盖升级</b> · v' + m.installed + ' → v' + m.version + '（配置保留）';
      }
    } else {
      txt.innerHTML = '<b>首次安装</b> · 将安装 v' + m.version + '（不影响系统内其他软件）';
    }
    if (date) {
      var d = document.createElement('span');
      d.className = 'ii-date';
      d.textContent = '安装包构建时间 ' + date;
      txt.appendChild(d);
    }
  }

  /* ── host → web ── */
  if (host) {
    host.addEventListener('message', function (e) {
      var m;
      try { m = (typeof e.data === 'string') ? JSON.parse(e.data) : e.data; } catch (err) { return; }
      if (!m || typeof m !== 'object') return;
      switch (m.type) {
        case 'meta':
          if (m.mode === 'uninstall') {
            renderUninstallMeta(m);
            break;
          }
          state.version = m.version || '';
          state.exeName = m.exeName || state.exeName;
          $('floorTag').textContent = 'ANIBOX · V' + m.version;
          setMode(m.mode || 'user');
          setPath(m.path || '');
          $('dirHint').textContent = '点「浏览…」选择安装文件夹';
          renderInstallInfo(m);
          break;
        case 'autoUninstall':
          /* E2E: 等确认页渲染一帧后自动开始(默认按 --purge 勾选状态) */
          setTimeout(function () { startUninstall(String(m.purge) === '1'); }, 900);
          break;
        case 'folder':
          if (m.path) setPath(m.path);
          if (m.warn) toast(m.warn);
          break;
        case 'progress':
          setProgress(m.pct, m.phase, m.file);
          break;
        case 'installed':
          setProgress(100, 'done', '安装完成');
          state.installing = false;
          show('finish');
          break;
        case 'uninstalled':
          state.uninstalling = false;
          state.uninstalled = true;
          setProgress(100, 'done', '卸载完成');
          $('unDoneSub').innerHTML = String(m.purge) === '1'
            ? 'AniBox 已从这台电脑移除<br>用户数据已删除'
            : 'AniBox 已从这台电脑移除<br>用户数据已保留，重新安装后可继续使用';
          show('unfinish');
          break;
        case 'error':
          if (state.uninstallMode) {
            state.uninstalling = false;
            $('errMsg').textContent = m.msg || '卸载器遇到问题，请重试或手动删除安装目录。';
            show('error');
          } else {
            state.installing = false;
            toast(m.msg || '安装失败');
            show('options');
          }
          break;
      }
    });
    send({ type: 'ready' });
    /* ?tour=1 自动巡演: 截图验收用(含协议弹层一帧) */
    if (/[?&]tour=1/.test(location.search)) {
      setTimeout(function () { openLicense(); }, 1000);
      setTimeout(function () { closeLicense(true); }, 2300);
      setTimeout(function () { $('btnWelcomeNext').disabled = false; $('agreeRow').classList.add('on'); state.agreed = true; $('btnWelcomeNext').click(); }, 2600);
      setTimeout(function () { $('btnInstall').click(); }, 4000);
    }
    } else {
      /* 浏览器直接打开 www/ 时给演示数据 */
      $('floorTag').textContent = 'ANIBOX · V4.0.0';
      setPath('C:\\Users\\demo\\AppData\\Local\\AniBox');
      setMode('user');
    }
})();

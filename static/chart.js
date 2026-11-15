// Crosshair + tooltip voor de prijsgrafiek. De grafiek zelf is server-side SVG;
// zonder JavaScript blijft alles leesbaar via de tabel eronder.
(function () {
  document.querySelectorAll("[data-chart]").forEach(function (root) {
    var svg = root.querySelector("svg");
    var dataEl = root.querySelector(".chart-data");
    var tip = root.querySelector(".chart-tip");
    if (!svg || !dataEl || !tip) return;
    var points;
    try { points = JSON.parse(dataEl.textContent); } catch (e) { return; }
    if (!points.length) return;
    var hover = svg.querySelector(".hover");
    var line = svg.querySelector(".crosshair");
    var dot = svg.querySelector(".hover-dot");
    var tipPrice = tip.querySelector(".tip-price");
    var tipWhen = tip.querySelector(".tip-when");
    var current = -1;

    function toSvgX(clientX) {
      var pt = svg.createSVGPoint();
      pt.x = clientX; pt.y = 0;
      var m = svg.getScreenCTM();
      return m ? pt.matrixTransform(m.inverse()).x : 0;
    }
    function indexAt(x) {
      for (var i = 0; i < points.length; i++) {
        if (x < points[i].x2 || i === points.length - 1) return i;
      }
      return points.length - 1;
    }
    function show(i, x) {
      current = i;
      var p = points[i];
      if (x === undefined) x = (p.x + p.x2) / 2;
      x = Math.max(points[0].x, Math.min(points[points.length - 1].x2, x));
      line.setAttribute("x1", x); line.setAttribute("x2", x);
      dot.setAttribute("cx", x); dot.setAttribute("cy", p.y);
      hover.setAttribute("visibility", "visible");
      tipPrice.textContent = p.price;
      tipWhen.textContent = p.from === p.until ? p.from : p.from + " tot " + p.until;
      tip.hidden = false;
      var vb = svg.viewBox.baseVal;
      var rel = x / vb.width;
      var px = rel * svg.getBoundingClientRect().width + svg.getBoundingClientRect().left - root.getBoundingClientRect().left;
      var half = tip.offsetWidth / 2 + 4;
      px = Math.max(half, Math.min(root.clientWidth - half, px));
      tip.style.left = px + "px";
    }
    function hide() {
      hover.setAttribute("visibility", "hidden");
      tip.hidden = true;
    }
    svg.addEventListener("pointermove", function (e) {
      var x = toSvgX(e.clientX);
      show(indexAt(x), x);
    });
    svg.addEventListener("pointerleave", hide);
    svg.addEventListener("blur", hide);
    svg.addEventListener("focus", function () { show(points.length - 1); });
    svg.addEventListener("keydown", function (e) {
      if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        e.preventDefault();
        var i = current < 0 ? points.length - 1 : current + (e.key === "ArrowLeft" ? -1 : 1);
        show(Math.max(0, Math.min(points.length - 1, i)));
      } else if (e.key === "Escape") {
        hide();
      }
    });
  });
})();

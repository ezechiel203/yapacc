/* yapacc — website script.
   Copyright (C) 2026 Alexandre BETRY. SPDX-License-Identifier: GPL-2.0-only
   Everything here is an enhancement: the pages read the same without it.
   No dependency, no request other than the handbook's own search index. */
(function () {
  "use strict";
  var doc = document;
  var root = doc.documentElement;
  root.classList.add("js");
  var calm = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  function all(sel, el) {
    return Array.prototype.slice.call((el || doc).querySelectorAll(sel));
  }

  /* -------------------------------------------------------------- theme */
  var toggle = doc.getElementById("theme-toggle");
  if (toggle) {
    toggle.addEventListener("click", function () {
      var dark =
        root.dataset.theme === "dark" ||
        (!root.dataset.theme && window.matchMedia("(prefers-color-scheme: dark)").matches);
      root.dataset.theme = dark ? "light" : "dark";
      try {
        localStorage.setItem("yapacc-theme", root.dataset.theme);
      } catch (e) {
        /* private mode: the choice lasts for this page only */
      }
    });
  }

  /* --------------------------------------------------------------- tabs */
  all("[data-tabs]").forEach(function (group) {
    var buttons = all("[role=tab]", group);
    var panels = buttons.map(function (b) {
      return doc.getElementById(b.getAttribute("aria-controls"));
    });
    function select(i, focus) {
      buttons.forEach(function (b, j) {
        b.setAttribute("aria-selected", j === i ? "true" : "false");
        b.tabIndex = j === i ? 0 : -1;
        if (panels[j]) panels[j].hidden = j !== i;
      });
      if (focus) buttons[i].focus();
      group.dispatchEvent(new CustomEvent("tab", { detail: { index: i, panel: panels[i] } }));
    }
    buttons.forEach(function (b, i) {
      b.addEventListener("click", function () {
        select(i, false);
      });
      b.addEventListener("keydown", function (e) {
        var n = buttons.length;
        if (e.key === "ArrowRight") select((i + 1) % n, true);
        else if (e.key === "ArrowLeft") select((i + n - 1) % n, true);
        else if (e.key === "Home") select(0, true);
        else if (e.key === "End") select(n - 1, true);
        else return;
        e.preventDefault();
      });
    });
    group.selectTab = select;
  });

  /* ------------------------------------------------- the typed terminal */
  // The capture is already in the page. Playing it hides the lines, types
  // the command, then shows the output line by line.
  function play(panel) {
    if (calm || !panel || panel.dataset.played) return;
    panel.dataset.played = "1";
    var lines = all(".ln", panel);
    var cmd = lines[0] && lines[0].querySelector(".c");
    if (!cmd) return;
    var text = cmd.textContent;
    var rest = lines.slice(1);
    rest.forEach(function (l) {
      l.style.visibility = "hidden";
    });
    cmd.textContent = "";
    var caret = doc.createElement("span");
    caret.className = "caret";
    cmd.after(caret);
    var i = 0;
    (function type() {
      if (!panel.isConnected) return;
      if (i < text.length) {
        cmd.textContent += text.charAt(i++);
        setTimeout(type, 28 + Math.random() * 46);
        return;
      }
      setTimeout(function () {
        caret.remove();
        var j = 0;
        (function show() {
          if (j >= rest.length) return;
          rest[j++].style.visibility = "";
          setTimeout(show, 34);
        })();
      }, 320);
    })();
  }
  all(".term[data-tabs]").forEach(function (term) {
    term.addEventListener("tab", function (e) {
      play(e.detail.panel);
    });
    if ("IntersectionObserver" in window) {
      var seen = new IntersectionObserver(
        function (entries) {
          if (entries[0].isIntersecting) {
            play(term.querySelector(".term-panel:not([hidden])"));
            seen.disconnect();
          }
        },
        { threshold: 0.35 }
      );
      seen.observe(term);
    }
  });

  /* --------------------------------------------------------------- copy */
  function copyText(figure) {
    var cmds = all(".ln.cmd .c", figure);
    if (cmds.length) {
      return cmds
        .map(function (c) {
          return c.textContent;
        })
        .join("\n");
    }
    return figure.querySelector("code").textContent.replace(/\n$/, "");
  }
  all("figure.code").forEach(function (figure) {
    if (!navigator.clipboard) return;
    var b = doc.createElement("button");
    b.type = "button";
    b.className = "copy";
    b.textContent = "Copy";
    b.setAttribute("aria-label", "Copy to clipboard");
    b.addEventListener("click", function () {
      navigator.clipboard.writeText(copyText(figure)).then(function () {
        b.textContent = "Copied";
        setTimeout(function () {
          b.textContent = "Copy";
        }, 1400);
      });
    });
    figure.appendChild(b);
  });

  /* ------------------------------------------------------------- reveal */
  var reveals = all(".reveal");
  if (reveals.length && "IntersectionObserver" in window && !calm) {
    var io = new IntersectionObserver(
      function (entries) {
        entries.forEach(function (en) {
          if (en.isIntersecting) {
            en.target.classList.add("in");
            io.unobserve(en.target);
          }
        });
      },
      { rootMargin: "0px 0px -8% 0px", threshold: 0.05 }
    );
    reveals.forEach(function (el) {
      io.observe(el);
    });
  } else {
    reveals.forEach(function (el) {
      el.classList.add("in");
    });
  }

  /* --------------------------------------------- handbook: side contents */
  var navBtn = doc.getElementById("nav-toggle");
  var nav = doc.getElementById("doc-nav");
  if (navBtn && nav) {
    navBtn.addEventListener("click", function () {
      var open = nav.classList.toggle("open");
      navBtn.setAttribute("aria-expanded", open ? "true" : "false");
    });
  }

  var tocLinks = all(".doc-toc a");
  if (tocLinks.length && "IntersectionObserver" in window) {
    var byId = {};
    tocLinks.forEach(function (a) {
      byId[decodeURIComponent(a.hash.slice(1))] = a;
    });
    var spy = new IntersectionObserver(
      function (entries) {
        entries.forEach(function (en) {
          if (!en.isIntersecting) return;
          tocLinks.forEach(function (a) {
            a.classList.remove("active");
          });
          var a = byId[en.target.id];
          if (a) a.classList.add("active");
        });
      },
      { rootMargin: "-80px 0px -70% 0px" }
    );
    all(".prose h2[id], .prose h3[id]").forEach(function (h) {
      spy.observe(h);
    });
  }

  /* ----------------------------------------------------------- search */
  var dialog = doc.getElementById("search");
  if (dialog && dialog.showModal) {
    var base = root.dataset.root || "";
    var input = dialog.querySelector("input");
    var list = dialog.querySelector(".search-results");
    var empty = dialog.querySelector(".search-empty");
    var index = null;
    var sel = 0;

    function esc(s) {
      return s.replace(/[&<>"]/g, function (c) {
        return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c];
      });
    }
    function open() {
      if (dialog.open) return;
      dialog.showModal();
      input.select();
      if (!index) {
        fetch(base + "assets/search.json")
          .then(function (r) {
            return r.json();
          })
          .then(function (j) {
            index = j;
            run();
          })
          .catch(function () {
            empty.textContent = "The search index could not be loaded.";
          });
      }
    }
    function snippet(text, words) {
      var low = text.toLowerCase();
      var at = -1;
      words.forEach(function (w) {
        var i = low.indexOf(w);
        if (i >= 0 && (at < 0 || i < at)) at = i;
      });
      var start = Math.max(0, at - 50);
      var cut = (start > 0 ? "…" : "") + text.slice(start, start + 170) + (text.length > start + 170 ? "…" : "");
      var out = esc(cut);
      words.forEach(function (w) {
        out = out.replace(new RegExp("(" + w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + ")", "gi"), "<mark>$1</mark>");
      });
      return out;
    }
    function run() {
      if (!index) return;
      var words = input.value.toLowerCase().split(/\s+/).filter(Boolean);
      list.innerHTML = "";
      sel = 0;
      if (!words.length) {
        empty.hidden = false;
        empty.textContent = "Type a command, a flag or a word: undo, --dryrun, snapshot…";
        return;
      }
      var hits = [];
      index.forEach(function (s) {
        var h = s.h.toLowerCase(), p = s.p.toLowerCase(), t = s.t.toLowerCase();
        var score = 0;
        for (var i = 0; i < words.length; i++) {
          var w = words[i], got = 0;
          if (h.indexOf(w) >= 0) got += 6;
          if (p.indexOf(w) >= 0) got += 3;
          if (t.indexOf(w) >= 0) got += 1;
          if (!got) return;
          score += got;
        }
        hits.push({ s: s, score: score });
      });
      hits.sort(function (a, b) {
        return b.score - a.score;
      });
      empty.hidden = hits.length > 0;
      if (!hits.length) empty.textContent = "Nothing in the handbook matches that.";
      hits.slice(0, 12).forEach(function (hit, i) {
        var s = hit.s;
        var li = doc.createElement("li");
        li.innerHTML =
          '<a href="' + esc(base + s.u) + '"' + (i === 0 ? ' class="sel"' : "") + ">" +
          "<small>" + esc(s.p) + "</small>" +
          "<b>" + esc(s.h || s.p) + "</b>" +
          "<p>" + snippet(s.t, words) + "</p></a>";
        list.appendChild(li);
      });
    }
    function move(d) {
      var links = all("a", list);
      if (!links.length) return;
      links[sel].classList.remove("sel");
      sel = (sel + d + links.length) % links.length;
      links[sel].classList.add("sel");
      links[sel].scrollIntoView({ block: "nearest" });
    }
    input.addEventListener("input", run);
    input.addEventListener("keydown", function (e) {
      if (e.key === "ArrowDown") move(1);
      else if (e.key === "ArrowUp") move(-1);
      else if (e.key === "Enter") {
        var a = all("a", list)[sel];
        if (a) location.href = a.href;
      } else return;
      e.preventDefault();
    });
    dialog.addEventListener("click", function (e) {
      if (e.target === dialog) dialog.close();
    });
    all("[data-search-open]").forEach(function (b) {
      b.addEventListener("click", open);
    });
    doc.addEventListener("keydown", function (e) {
      var typing = /^(INPUT|TEXTAREA|SELECT)$/.test(e.target.tagName) || e.target.isContentEditable;
      if ((e.key === "/" && !typing) || (e.key === "k" && (e.ctrlKey || e.metaKey))) {
        e.preventDefault();
        open();
      }
    });
  }
})();

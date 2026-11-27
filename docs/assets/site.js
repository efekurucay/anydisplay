const platformSwitch = document.querySelector(".platform-switch");

function showPlatform(platform) {
  document.querySelectorAll("[data-platform]").forEach((item) => {
    item.setAttribute("aria-pressed", String(item.dataset.platform === platform));
  });
  document.querySelectorAll("[data-guide]").forEach((guide) => {
    guide.hidden = guide.dataset.guide !== platform;
  });
  if (platformSwitch) platformSwitch.dataset.active = platform;
}

document.querySelectorAll("[data-platform]").forEach((button) => {
  button.addEventListener("click", () => showPlatform(button.dataset.platform));
});

// Open the setup steps for the visitor's own computer.
const os = navigator.userAgentData?.platform || navigator.platform || "";
if (platformSwitch && /win/i.test(os)) showPlatform("windows");

const reduceMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;

// Spin each digit of the measurement into place, like a mechanical counter.
const roll = document.querySelector("[data-roll]");
if (roll && !reduceMotion && "IntersectionObserver" in window) {
  const value = roll.firstChild.textContent.trim();
  const label = document.createElement("span");
  label.className = "sr-only";
  label.textContent = value;
  const digits = document.createElement("span");
  digits.className = "roll";
  digits.setAttribute("aria-hidden", "true");
  // Two runs of 0-9 so every column travels about five digits, including zeros.
  const columns = [...value].map((digit, index) => {
    const column = document.createElement("span");
    column.className = "roll-col";
    column.dataset.target = String(10 + Number(digit));
    column.style.translate = `0 ${-(5 + Number(digit))}em`;
    column.style.transitionDelay = `${index * 90}ms`;
    for (let n = 0; n < 20; n++) {
      const cell = document.createElement("span");
      cell.textContent = String(n % 10);
      column.append(cell);
    }
    digits.append(column);
    return column;
  });
  roll.firstChild.replaceWith(label, digits);
  new IntersectionObserver(
    (entries, observer) => {
      if (!entries[0].isIntersecting) return;
      columns.forEach((column) => {
        column.style.translate = `0 ${-column.dataset.target}em`;
      });
      observer.disconnect();
    },
    { threshold: 0.6 },
  ).observe(roll);
}

// After a download starts, say what to do next.
const toastRegion = document.querySelector(".toast-region");
const nextSteps = {
  android: {
    title: "Downloading the Android app",
    body: "Open the APK and allow installs from your browser when Android asks.",
  },
  mac: {
    title: "Downloading for Mac",
    body: "Next: move it to Applications, then allow Screen Recording and Accessibility.",
    platform: "mac",
  },
  windows: {
    title: "Downloading for Windows",
    body: "Next: run the installer, then connect over USB or Wi-Fi.",
    platform: "windows",
  },
};
let dismissCurrentToast = null;

function showToast({ title, body, platform }) {
  dismissCurrentToast?.();
  const toast = document.createElement("div");
  toast.className = "toast";
  const dot = document.createElement("span");
  dot.className = "status-dot";
  const text = document.createElement("div");
  const heading = document.createElement("strong");
  heading.textContent = title;
  const detail = document.createElement("p");
  detail.textContent = body;
  text.append(heading, detail);
  const close = document.createElement("button");
  close.type = "button";
  close.setAttribute("aria-label", "Dismiss");
  close.textContent = "×";
  toast.append(dot, text, close);
  if (platform) {
    const link = document.createElement("a");
    link.href = "#setup";
    link.textContent = "Show the setup steps";
    link.addEventListener("click", () => {
      showPlatform(platform);
      dismiss();
    });
    toast.append(link);
  }

  // The timer pauses while the toast is hovered, focused or the tab is hidden.
  let remaining = 7000;
  let startedAt = 0;
  let timer = 0;
  const held = new Set();
  const run = () => {
    if (held.size || document.hidden) return;
    startedAt = Date.now();
    timer = setTimeout(dismiss, remaining);
  };
  const pause = () => {
    if (!timer) return;
    clearTimeout(timer);
    timer = 0;
    remaining -= Date.now() - startedAt;
  };
  const hold = (event) => {
    held.add(event.type === "focusin" ? "focus" : "pointer");
    pause();
  };
  const release = (event) => {
    held.delete(event.type === "focusout" ? "focus" : "pointer");
    run();
  };
  const onVisibility = () => (document.hidden ? pause() : run());

  function dismiss() {
    clearTimeout(timer);
    document.removeEventListener("visibilitychange", onVisibility);
    if (dismissCurrentToast === dismiss) dismissCurrentToast = null;
    toast.classList.add("leaving");
    setTimeout(() => toast.remove(), 200);
  }

  toast.addEventListener("pointerenter", hold);
  toast.addEventListener("pointerleave", release);
  toast.addEventListener("focusin", hold);
  toast.addEventListener("focusout", release);
  close.addEventListener("click", dismiss);
  document.addEventListener("visibilitychange", onVisibility);
  dismissCurrentToast = dismiss;
  toastRegion.append(toast);
  run();
}

if (toastRegion) {
  document.querySelectorAll("[data-download]").forEach((link) => {
    link.addEventListener("click", () => showToast(nextSteps[link.dataset.download]));
  });
}

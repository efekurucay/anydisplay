document.querySelectorAll("[data-platform]").forEach((button) => {
  button.addEventListener("click", () => {
    const platform = button.dataset.platform;
    document.querySelectorAll("[data-platform]").forEach((item) => {
      item.setAttribute("aria-pressed", String(item === button));
    });
    document.querySelectorAll("[data-guide]").forEach((guide) => {
      guide.hidden = guide.dataset.guide !== platform;
    });
  });
});

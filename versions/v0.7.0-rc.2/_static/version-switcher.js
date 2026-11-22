// The shared asset URL identifies the site root, including a repository prefix.
(() => {
  const site = new URL("../", document.currentScript.src);
  async function addVersionSwitcher() {
    try {
      const response = await fetch(new URL("versions.json", site));
      if (!response.ok) return;
      const manifest = await response.json();
      const path = window.location.pathname.slice(site.pathname.length);
      const current = manifest.versions.find(entry => path.startsWith(entry.path))
        || manifest.versions.find(entry => entry.path === manifest.default);
      if (!current) return;
      const container = document.createElement("div");
      container.className = "oe-version-switcher";
      const label = document.createElement("label");
      label.textContent = "Documentation version";
      label.htmlFor = "oe-docs-version";
      const select = document.createElement("select");
      select.id = label.htmlFor;
      for (const entry of manifest.versions) {
        const option = document.createElement("option");
        option.value = entry.path;
        option.textContent = entry.label;
        option.selected = entry.path === current.path;
        select.append(option);
      }
      const status = document.createElement("p");
      status.textContent = current.kind === "development"
        ? "Development docs may describe unreleased changes."
        : current.kind === "prerelease" ? "You are reading prerelease documentation." : "";
      const allVersions = document.createElement("a");
      allVersions.href = new URL("versions/", site).href;
      allVersions.textContent = "All versions";
      container.append(label, select, status, allVersions);
      const sidebar = document.querySelector(".sidebar-scroll");
      (sidebar || document.body).prepend(container);
      select.addEventListener("change", async () => {
        const base = new URL(select.value, site);
        const relative = path.startsWith(current.path) ? path.slice(current.path.length) : path;
        const target = new URL(relative || "index.html", base);
        target.hash = window.location.hash;
        try {
          const exists = await fetch(target, { method: "HEAD" });
          window.location.assign(exists.ok ? target.href : base.href);
        } catch {
          window.location.assign(base.href);
        }
      });
    } catch {
      // Local/offline Sphinx builds remain usable without a deployed manifest.
    }
  }
  addVersionSwitcher();
})();

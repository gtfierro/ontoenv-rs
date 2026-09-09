// Pre-expand every section in the sidebar navigation.
//
// Furo renders the full toctree but collapses each expandable node with an
// unchecked `.toctree-checkbox`. Checking them all on load shows the whole
// structure up front while leaving the toggles fully functional — a reader can
// still collapse a section they are not interested in.
document.addEventListener("DOMContentLoaded", function () {
  document
    .querySelectorAll(".sidebar-tree .toctree-checkbox")
    .forEach(function (checkbox) {
      checkbox.checked = true;
    });
});

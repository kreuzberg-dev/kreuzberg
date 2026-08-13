const path = require("path");
const { task, src, dest } = require("gulp");

// Copy node icons (svg/png) into dist so n8n can serve them alongside the
// compiled node. n8n resolves `icon: 'file:xberg.svg'` relative to the node's
// compiled directory, so icons must sit next to Xberg.node.js in dist. ~keep
task("build:icons", copyIcons);

function copyIcons() {
  const nodeSource = path.resolve("nodes", "**", "*.{png,svg}");
  const nodeDestination = path.resolve("dist", "nodes");

  return src(nodeSource).pipe(dest(nodeDestination));
}

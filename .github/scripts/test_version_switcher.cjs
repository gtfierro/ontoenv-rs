const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync('docs/_static/version-switcher.js', 'utf8');
const manifest = {default:'versions/v0.6.4/', versions:[
 {path:'versions/v0.7.0-rc.1/',label:'0.7.0-rc.1 (prerelease)',kind:'prerelease'},
 {path:'versions/v0.6.4/',label:'0.6.4 (stable)',kind:'stable'},
 {path:'dev/',label:'Development (main)',kind:'development'}]};
async function check(prefix, path, exists, expectedKind, expectedLocation) {
 const elements=[]; const mounted=[]; const assigned=[];
 const createElement = tag => {
  const element = {tag,children:[],listeners:{},append(...children){this.children.push(...children)},addEventListener(event, callback){this.listeners[event]=callback}};
  elements.push(element);return element;
 };
 const sidebar={prepend(element){mounted.push(element)}};
 const context={URL, document:{currentScript:{src:`https://example.org${prefix}_static/version-switcher.js`},createElement,querySelector(){return sidebar},body:sidebar},window:{location:{pathname:prefix+path,hash:'#api',assign(url){assigned.push(url)}}},async fetch(url,options){return options ? {ok:exists} : {ok:true,json:async()=>manifest}}};
 vm.runInNewContext(source,context);
 await new Promise(resolve=>setImmediate(resolve));
 assert.equal(mounted.length,1);
 const selected=elements.find(e=>e.tag==='select');
 assert.equal(selected.children.filter(e=>e.selected).length,1);
 const status=elements.find(e=>e.tag==='p').textContent;
 assert.equal(status.includes(expectedKind),true);
 selected.value='versions/v0.6.4/';
 await selected.listeners.change();
 assert.equal(assigned[0],`https://example.org${prefix}${expectedLocation}`);
}
(async()=>{
 await check('/','versions/v0.7.0-rc.1/reference/api.html',true,'prerelease','versions/v0.6.4/reference/api.html#api');
 await check('/ontoenv-rs/','dev/reference/api.html',false,'Development','versions/v0.6.4/');
 await check('/','reference/api.html',true,'','versions/v0.6.4/reference/api.html#api');
 console.log('Version selector passed: RC/stable/development labels, nested routes, repository prefix, missing-page fallback.');
})().catch(error=>{console.error(error);process.exitCode=1});

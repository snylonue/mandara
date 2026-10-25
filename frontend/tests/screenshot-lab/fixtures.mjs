// Synthetic publication content, not copyrighted book excerpts.
export const fixtures = [
  {
    id: "plain",
    body: "<h2>阅读器排版对照</h2><p>这是用于自动测试的正文。中文标点、换行位置和段落间距都应该保持一致。</p><p>中日文混排：これは表示確認のための文章です。English words and 1234567890.</p>",
  },
  {
    id: "font-variants",
    css: '#capture{font-family:"Noto Serif SC Variable",serif} .light{font-weight:300}.heavy{font-weight:800}.italic{font-style:italic}',
    body: '<p class="light">清晨的风吹过书页，文字应当保留原来的字体。</p><p class="heavy">粗体与普通字体：命运、偶然、閱讀。</p><p class="italic">斜体 English italic 与中文混排。</p><p>ひらがな カタカナ 日本語 简体中文 繁體中文。</p>',
  },
  {
    id: "ruby-bidi",
    body: '<p><ruby>汉字<rt>かんじ</rt></ruby>与<ruby>阅读<rt>yuè dú</rt></ruby>，强调<strong>粗体</strong>、<em>斜体</em>和<sup>上标</sup><sub>下标</sub>。</p><p dir="rtl">مرحبا بالعالم <bdi>中文 ABC 123</bdi> שלום עולם</p><p>表情 👩‍💻 👨‍👩‍👧‍👦 🇨🇳 é ﬃ 字符不能被拆开。</p>',
  },
  {
    id: "vertical",
    css: "#capture{height:320px;writing-mode:vertical-rl;text-orientation:mixed}p{margin:0 0 0 18px}",
    body: "<p><ruby>纵排<rt>たてがき</rt></ruby>正文，标点与换行应当一致。</p><p>这是第二段。ABC 123 与日文「こんにちは」。</p><p>第三段包含<strong>强调</strong>和<em>斜体</em>。</p>",
  },
  {
    id: "cascade-pseudo",
    css: '#capture{--ink:#8d2c48;counter-reset:line 4;background:#f5edd8}p{color:var(--ink);counter-increment:line}p::before{content:counter(line)"、";font-weight:800}p::first-letter{font-size:1.4em}p::after{content:"〔完〕";color:#287570}',
    body: '<p>伪元素、计数器和继承变量。</p><p>第二段仍然应当显示正确编号。</p><p style="color:#28488a;text-decoration:underline dotted">内联样式与装饰线。</p>',
  },
  {
    id: "grid-flex-table",
    css: ".grid{display:grid;grid-template-columns:1fr 2fr;gap:9px}.grid>div{border:2px solid #927;padding:9px}.flex{display:flex;gap:12px;align-items:baseline}table{border-collapse:collapse;width:100%}td,th{border:1px solid #278;padding:6px}",
    body: '<div class="grid"><div>短栏</div><div>宽栏文字应当在相同的位置换行。</div></div><div class="flex"><strong>标题</strong><span>基线对齐与间隙</span></div><table><tr><th>项目</th><th>说明</th></tr><tr><td>排版</td><td>表格与边框不能错位。</td></tr></table>',
  },
  {
    id: "paint-effects",
    css: ".paint{padding:14px;margin:12px;background:linear-gradient(110deg,#ffe79a,#9ed9ea);border-radius:18px;border:3px solid #478;box-shadow:3px 4px 6px #777;transform:rotate(1deg)}.clip{width:120px;height:70px;background:#95b;clip-path:polygon(0 0,100% 0,75% 100%,25% 100%);opacity:.7}",
    body: '<div class="paint">渐变、圆角、阴影和旋转。</div><div class="clip"></div><p style="text-shadow:1px 1px 2px #c99">透明度与文字阴影。</p>',
  },
  {
    id: "responsive",
    css: "#capture{container-type:inline-size}.responsive{font-size:3.2cqw;min-height:18vh;border:2px solid #287}@media(max-width:450px){.responsive{font-size:22px;background:#daf3de}}@media(min-width:451px){.responsive{background:#e1e5ff}}",
    body: '<div class="responsive">媒体查询、容器单位、视口高度。窄屏和宽屏必须各自匹配。</div>',
  },
  {
    id: "image-svg",
    css: 'svg image[*|href="/api/images/lab.png"]{opacity:.55}',
    body: '<p>同源插图、背景图与内嵌 SVG：</p><img width="96" height="64" src="/api/images/lab.png"><span style="display:inline-block;width:96px;height:64px;background:url(/api/images/lab.png) center/cover"></span><svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="96" height="64"><image width="96" height="64" xlink:href="/api/images/lab.png"/></svg>',
  },
  {
    id: "semantic-svg",
    body: '<section epub:type="chapter"><p>语义属性与矢量插图。</p><svg xmlns="http://www.w3.org/2000/svg" width="150" height="80"><defs><linearGradient id="g"><stop stop-color="#fa7"/><stop offset="1" stop-color="#48c"/></linearGradient></defs><rect width="150" height="80" rx="15" fill="url(#g)"/><text x="15" y="48" font-size="22">图中文字</text></svg></section>',
  },
  {
    id: "selection-siblings",
    selection: "#selection",
    css: '#capture{padding:0}p{margin:0;padding:16px;min-height:90px;box-sizing:border-box}p:nth-child(2){color:#a21;background:#e6efca;font-weight:800}p:first-child::before{content:"首段："}',
    body: '<p>未选中的前文。</p><p id="selection">只选择中间这一段，不能改变字体、背景和伪元素。</p><p>未选中的后文。</p>',
  },
  {
    id: "selection-counters",
    selection: "#selection",
    css: '#capture{padding:0;counter-reset:para 7}p{counter-increment:para;margin:0;padding:16px;min-height:90px;box-sizing:border-box}p::before{content:"第 "counter(para)" 段："}',
    body: '<p>前文。</p><p id="selection">选段应当保留它在原文中的计数。</p><p>后文。</p>',
  },
  {
    id: "partial-inline",
    selection: "#selection",
    range: { start: "#start", startOffset: 7, end: "#end", endOffset: 5 },
    css: 'p:nth-child(2){font-size:20.25px;font-weight:700;line-height:1.83;color:#9e244e}p::before{content:"段首："}',
    body: '<p>不能输出的前一段。</p><p id="selection"><span id="start">不能输出的开头这部分选中</span><strong>强调和行内排版保持不变</strong><span id="end">选中的结尾不能输出的后文</span></p><p>不能输出的后一段。</p>',
  },
  {
    id: "long",
    body: Array.from(
      { length: 36 },
      (_, i) =>
        "<p>第" +
        (i + 1) +
        "段：这是用于长截图的测试文本。滚动区域以外的文字也要完整保留，不能截断、重复或改变行宽。</p>",
    ).join(""),
  },
  {
    id: "txt",
    format: "txt",
    body: '<h2>纯文本阅读器</h2><p>纯文本路径也必须使用阅读器的排版，而不是另一套画布排版。</p><blockquote>这是一段引用文字。</blockquote><p><strong>强调</strong>、<code>const value = 42;</code> 与链接<a href="#">测试链接</a>。</p>',
  },
  {
    id: "delayed-image",
    body: '<p>延迟加载资源不能在结果里消失。</p><img width="96" height="64" src="/api/images/lab.png?delay=350">',
  },
  {
    id: "overflow-clip",
    css: ".scroll{height:100px;overflow:auto;border:2px solid #843}.outside{position:relative;left:12px;width:calc(100% - 12px);background:#e8d6f3}",
    body: '<div class="scroll"><p>滚动容器第一行。</p><p>第二行。</p><p>第三行。</p><p>隐藏在容器外的第四行。</p></div><div class="outside">相对定位与内容裁剪。</div>',
  },
  {
    id: "dark",
    css: "#capture{background:#20242d;color:#e8e1d3}a{color:#f8bb72}blockquote{border-left:3px solid #a76;padding-left:12px;color:#c8b9a0}",
    body: '<h2>夜间主题</h2><p>深色背景上的文字与<a href="#">链接颜色</a>。</p><blockquote>引用的颜色、边框和间距。</blockquote>',
  },
  {
    id: "missing-image",
    broken: true,
    body: '<p>缺失资源：记录报错还是静默缺图。</p><img width="96" height="64" alt="缺失插图" src="/api/images/missing.png">',
  },
];
export const baseCSS =
  "html{background:#fff!important}body{margin:0!important;padding:0!important}#capture{display:flow-root;box-sizing:border-box;width:100%;padding:16px;background:#fff;color:#222}p{margin:0 0 12px}#capture>:last-child{margin-bottom:0}";

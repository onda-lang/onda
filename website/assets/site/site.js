(() => {
  const root = document.documentElement;
  const themeButton = document.querySelector('.theme-toggle');
  const themeColor = document.querySelector('meta[name="theme-color"]');
  const siteIcon = document.querySelector('#site-icon');
  const colorScheme = window.matchMedia('(prefers-color-scheme: dark)');

  const resolvedTheme = () => {
    const selected = root.dataset.theme || 'auto';
    return selected === 'auto' ? (colorScheme.matches ? 'dark' : 'light') : selected;
  };

  const syncTheme = () => {
    const selected = root.dataset.theme || 'auto';
    const resolved = resolvedTheme();
    themeColor?.setAttribute('content', resolved === 'dark' ? '#0c1e33' : '#f5f9ff');
    if (siteIcon) siteIcon.href = siteIcon.dataset[resolved];
    if (themeButton) {
      themeButton.title = `Theme: ${selected}`;
      themeButton.setAttribute('aria-label', `Theme: ${selected}. Change color theme`);
    }
  };

  syncTheme();
  colorScheme.addEventListener('change', syncTheme);
  themeButton?.addEventListener('click', () => {
    const next = resolvedTheme() === 'dark' ? 'light' : 'dark';
    root.dataset.theme = next;
    localStorage.setItem('onda-theme', next);
    syncTheme();
  });

  const menuButton = document.querySelector('.menu-toggle');
  const menu = document.querySelector('#main-nav');
  menuButton?.addEventListener('click', () => {
    const open = menu.classList.toggle('open');
    menuButton.setAttribute('aria-expanded', String(open));
  });

  const sectionWords = new Set([
    'ins', 'inputs', 'outs', 'outputs', 'params', 'kins', 'kouts',
    'buffers', 'events', 'init', 'block', 'sample', 'graph',
  ]);
  const declarationWords = new Set([
    'const', 'def', 'event', 'proc', 'processor', 'struct', 'namespace',
  ]);
  const nameFollowing = new Set(['def', 'event', 'proc', 'processor', 'struct', 'namespace']);
  const keywordWords = new Set([
    'if', 'elif', 'else', 'for', 'in', 'while', 'loop', 'break',
    'continue', 'return', 'assert', 'import', 'include', 'use', 'pub',
    'as', 'pin',
  ]);
  const typeWords = new Set(['f32', 'f64', 'i32', 'i64', 'bool', 'buffer']);
  const constantWords = new Set([
    'true', 'false', 'PI', 'TWO_PI', 'TWOPI', 'SR', 'SAMPLE_RATE',
    'SAMPLERATE', 'HOST_SR', 'HOST_SAMPLE_RATE', 'HOST_SAMPLERATE',
    'BS', 'BLOCK_SIZE', 'BLOCKSIZE',
  ]);

  const highlightOnda = (code) => {
    const source = code.textContent || '';
    const fragment = document.createDocumentFragment();
    let offset = 0;
    let lineStart = true;
    let expectedName = false;

    const append = (value, className) => {
      if (!className) {
        fragment.append(document.createTextNode(value));
        return;
      }
      const span = document.createElement('span');
      span.className = className;
      span.textContent = value;
      fragment.append(span);
    };

    while (offset < source.length) {
      const rest = source.slice(offset);
      let match;

      if ((match = rest.match(/^\s+/))) {
        append(match[0]);
        if (match[0].includes('\n')) {
          lineStart = true;
        }
        offset += match[0].length;
        continue;
      }
      if (rest[0] === '#') {
        const end = rest.indexOf('\n');
        const value = end < 0 ? rest : rest.slice(0, end);
        append(value, 'syntax-comment');
        offset += value.length;
        continue;
      }
      if (rest[0] === '"') {
        match = rest.match(/^"(?:\\.|[^"\\])*"?/);
        append(match[0], 'syntax-string');
        offset += match[0].length;
        lineStart = false;
        continue;
      }
      if ((match = rest.match(/^\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/))) {
        append(match[0], 'syntax-number');
        offset += match[0].length;
        lineStart = false;
        continue;
      }
      if ((match = rest.match(/^[A-Za-z_][A-Za-z0-9_]*/))) {
        const word = match[0];
        let className = '';
        if (lineStart && sectionWords.has(word)) className = 'syntax-section';
        else if (declarationWords.has(word)) className = 'syntax-declaration';
        else if (expectedName) className = 'syntax-function';
        else if (keywordWords.has(word)) className = 'syntax-keyword';
        else if (typeWords.has(word)) className = 'syntax-type';
        else if (constantWords.has(word)) className = 'syntax-constant';
        else if (/^(?:in|out|kout|param|kin|buf)\d+$/.test(word)) className = 'syntax-constant';
        else if (/^\s*\(/.test(rest.slice(word.length))) className = 'syntax-function';
        append(word, className);
        expectedName = nameFollowing.has(word);
        offset += word.length;
        lineStart = false;
        continue;
      }
      if ((match = rest.match(/^(?:>>\[[^\]\n]+\]|<<\[[^\]\n]+\]|\.\.=|\.\.|>>|<<|==|!=|<=|>=|&&|\|\||::|@(?:sample|block)|[+\-*/%=&|^~!<>])/))) {
        append(match[0], 'syntax-operator');
        offset += match[0].length;
        lineStart = false;
        continue;
      }

      append(rest[0]);
      offset += 1;
      lineStart = false;
    }

    code.replaceChildren(fragment);
  };

  document.querySelectorAll('code.language-onda').forEach(highlightOnda);

  document.querySelectorAll('.prose pre, .home-example pre').forEach((pre) => {
    const button = document.createElement('button');
    button.className = 'copy-button';
    button.type = 'button';
    button.textContent = 'copy';
    button.addEventListener('click', async () => {
      const code = pre.querySelector('code')?.textContent || '';
      await navigator.clipboard.writeText(code);
      button.textContent = 'copied';
      window.setTimeout(() => { button.textContent = 'copy'; }, 1400);
    });
    pre.appendChild(button);
  });

  const toc = document.querySelector('#page-toc');
  const headings = [...document.querySelectorAll('.prose h2, .prose h3')];
  if (toc && headings.length) {
    headings.forEach((heading) => {
      if (!heading.id) return;
      const link = document.createElement('a');
      link.href = `#${heading.id}`;
      link.textContent = heading.textContent;
      link.dataset.level = heading.tagName.slice(1);
      toc.appendChild(link);
    });

    const links = [...toc.querySelectorAll('a')];
    const observer = new IntersectionObserver((entries) => {
      entries.forEach((entry) => {
        if (!entry.isIntersecting) return;
        links.forEach((link) => link.classList.toggle('active', link.hash === `#${entry.target.id}`));
      });
    }, { rootMargin: '-15% 0px -75%' });
    headings.forEach((heading) => observer.observe(heading));
  }
})();

import { highlightOnda } from "./onda-highlighter.js";

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

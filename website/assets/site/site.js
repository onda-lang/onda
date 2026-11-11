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
    menu.classList.toggle('open');
  });

  const header = document.querySelector('.site-header');
  if (header) {
    const mobile = window.matchMedia('(max-width: 880px)');
    let lastY = window.scrollY;
    const showHeader = () => header.classList.remove('site-header-hidden');

    window.addEventListener('scroll', () => {
      // Clamp elastic overscroll so bouncing at either edge cannot reverse direction.
      const maxY = Math.max(0, root.scrollHeight - window.innerHeight);
      const y = Math.max(0, Math.min(window.scrollY, maxY));
      if (!mobile.matches || y <= header.offsetHeight || menu?.classList.contains('open')) {
        showHeader();
      } else {
        // Accumulate small movements, but ignore finger jitter before changing state.
        if (Math.abs(y - lastY) < 6) return;
        header.classList.toggle('site-header-hidden', y > lastY);
      }
      lastY = y;
    }, { passive: true });

    header.addEventListener('focusin', showHeader);
    mobile.addEventListener('change', () => {
      lastY = window.scrollY;
      showHeader();
    });
  }

  document.querySelectorAll('code.language-onda').forEach(highlightOnda);

  document.querySelectorAll('.prose table').forEach((table) => {
    const wrapper = document.createElement('div');
    wrapper.className = 'table-scroll';
    table.before(wrapper);
    wrapper.appendChild(table);
  });

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

  // Build the contents first so permalink markers stay out of its labels.
  document.querySelectorAll('.prose :is(h1, h2, h3, h4, h5, h6)[id]').forEach((heading) => {
    const link = document.createElement('a');
    link.className = 'heading-anchor';
    link.href = `#${encodeURIComponent(heading.id)}`;
    link.textContent = '#';
    link.setAttribute('aria-label', `Permalink to ${heading.textContent.trim()}`);
    link.title = 'Link to this section';
    heading.appendChild(link);
  });
})();

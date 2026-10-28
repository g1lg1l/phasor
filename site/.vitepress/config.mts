import { defineConfig } from 'vitepress'
// written by scripts/site.py from the app's Help → Manual
import manual from './manual.mts'

export default defineConfig({
  title: 'Ravune',
  description: 'A modular DAW for deep sound design: the manual and reference.',
  base: '/ravune/',
  srcDir: 'src',
  cleanUrls: true,
  appearance: 'dark',
  head: [['link', { rel: 'icon', type: 'image/png', href: '/ravune/icon.png' }]],
  themeConfig: {
    logo: '/icon.png',
    nav: [
      { text: 'Manual', link: '/manual/your-first-beat' },
      { text: 'Reference', items: [
        { text: 'Sound library', link: '/sounds' },
        { text: 'Scripts', link: '/scripts' },
        { text: 'Agents (MCP)', link: '/agents' },
        { text: 'Extensions', link: '/extensions' },
        { text: 'Commands', link: '/commands' },
      ] },
    ],
    sidebar: [
      { text: 'Start', items: [
        { text: 'What is Ravune', link: '/introduction' },
        { text: 'Install and run', link: '/install' },
      ] },
      { text: 'Manual', items: manual },
      { text: 'Reference', items: [
        { text: 'Sound library', link: '/sounds' },
        { text: 'Scripts', link: '/scripts' },
        { text: 'Agents (MCP)', link: '/agents' },
        { text: 'Extensions', link: '/extensions' },
        { text: 'Commands', link: '/commands' },
      ] },
    ],
    socialLinks: [
      { icon: 'github', link: 'https://github.com/g1lg1l/ravune' },
      { icon: 'kofi', link: 'https://ko-fi.com/g1lg1l', ariaLabel: 'Support on Ko-fi' },
    ],
    search: { provider: 'local' },
    outline: { level: [2, 2], label: 'On this page' },
    footer: {
      message: 'MIT or Apache-2.0 · the built-in sounds are CC0',
      copyright: 'The Ravune contributors',
    },
  },
})

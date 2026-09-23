import { render } from 'solid-js/web';
import './styles/global.css';
import { App } from './App';

const root = document.getElementById('root');
if (root === null) throw new Error('找不到 #root 挂载点');

render(() => <App />, root);

/**
 * 开发期给自动化验证留一个把手（生产构建里整段被 tree-shake 掉）。
 *
 * 需要有它的原因是 **Vite 的 HMR 会让同一个模块出现两个实例**：热更新过的模块，
 * 其 import 说明符会被改写成带 `?t=` 的 URL，外部脚本再 `import('/src/state/store.ts')`
 * 拿到的是另一份全新实例 —— 读到的状态永远是空的，切会话也不生效（踩过好几次）。
 * 从应用自己的模块图里把句柄挂出来，就没有这个歧义了。
 */
if (import.meta.env.DEV) {
  void Promise.all([import('./state/store'), import('./state/events')]).then(([store, events]) => {
    (window as unknown as Record<string, unknown>)['__qqDebug'] = { store, events };
  });
}

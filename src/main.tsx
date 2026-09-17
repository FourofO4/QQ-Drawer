import { render } from 'solid-js/web';
import './styles/global.css';
import { App } from './App';

const root = document.getElementById('root');
if (root === null) throw new Error('找不到 #root 挂载点');

render(() => <App />, root);

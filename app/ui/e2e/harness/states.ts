import { mount } from 'svelte';
import '../../src/lib/tokens.css';
import States from './States.svelte';

const target = document.getElementById('app');
if (!target) throw new Error('Missing #app');
mount(States, { target });

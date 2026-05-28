import { atom } from 'nanostores'
import type { SnifferResource } from '../types'

export const $snifferResources = atom<SnifferResource[]>([])
export const $snifferActive = atom(true)

import type { RefObject } from 'react'
import { useLayoutEffect } from 'react'
import { useLocation } from 'react-router-dom'

/** Reset the shell's scroll container when navigation selects a new page. */
export function useResetScrollOnPath(scrollRef: RefObject<HTMLElement | null>) {
  const { pathname } = useLocation()

  useLayoutEffect(() => {
    const container = scrollRef.current
    if (container && pathname.length > 0) container.scrollTop = 0
  }, [pathname, scrollRef])
}

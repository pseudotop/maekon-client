/**
 *
 */

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { createTag, fetchTags, type Tag } from '../api/client'
import { elevation, iconSize, motion } from '../styles/tokens'
import { cn } from '../utils/cn'
import { getRandomTagColor, TAG_COLORS, TagBadge } from './TagBadge'
import { Divider, Input } from './ui'

interface TagInputProps {
  selectedTags: Tag[]
  onAddTag: (tag: Tag) => void
  onRemoveTag: (tag: Tag) => void
  placeholder?: string
}

export function TagInput({ selectedTags, onAddTag, onRemoveTag, placeholder }: TagInputProps) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [inputValue, setInputValue] = useState('')
  const [isOpen, setIsOpen] = useState(false)
  const [selectedColor, setSelectedColor] = useState(getRandomTagColor())
  const [highlightedIndex, setHighlightedIndex] = useState(-1)
  const inputRef = useRef<HTMLInputElement>(null)
  const dropdownRef = useRef<HTMLDivElement>(null)

  const { data: allTags = [] } = useQuery({
    queryKey: ['tags'],
    queryFn: fetchTags,
  })

  const createTagMutation = useMutation({
    mutationFn: createTag,
    onSuccess: (newTag) => {
      queryClient.invalidateQueries({ queryKey: ['tags'] })
      onAddTag(newTag)
      setInputValue('')
      setSelectedColor(getRandomTagColor())
    },
  })

  const filteredTags = allTags.filter((tag) => {
    const notSelected = !selectedTags.some((t) => t.id === tag.id)
    const matchesSearch = tag.name.toLowerCase().includes(inputValue.toLowerCase())
    return notSelected && matchesSearch
  })

  const exactMatch = allTags.find((tag) => tag.name.toLowerCase() === inputValue.toLowerCase())
  const canCreateNew = inputValue.trim() && !exactMatch

  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      if (
        dropdownRef.current &&
        !dropdownRef.current.contains(event.target as Node) &&
        inputRef.current &&
        !inputRef.current.contains(event.target as Node)
      ) {
        setIsOpen(false)
      }
    }

    document.addEventListener('mousedown', handleClickOutside)
    return () => document.removeEventListener('mousedown', handleClickOutside)
  }, [])

  const handleInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setInputValue(e.target.value)
    setHighlightedIndex(-1)
    if (!isOpen) setIsOpen(true)
  }

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      if (!isOpen) {
        setIsOpen(true)
      }
      setHighlightedIndex((prev) => (prev < filteredTags.length - 1 ? prev + 1 : 0))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setHighlightedIndex((prev) => (prev > 0 ? prev - 1 : filteredTags.length - 1))
    } else if (e.key === 'Enter') {
      e.preventDefault()
      if (highlightedIndex >= 0 && highlightedIndex < filteredTags.length) {
        handleSelectTag(filteredTags[highlightedIndex])
      } else if (canCreateNew) {
        createTagMutation.mutate({
          name: inputValue.trim(),
          color: selectedColor,
        })
      }
    } else if (e.key === 'Escape') {
      setIsOpen(false)
      setHighlightedIndex(-1)
    }
  }

  const handleSelectTag = (tag: Tag) => {
    onAddTag(tag)
    setInputValue('')
    setIsOpen(false)
    setHighlightedIndex(-1)
    inputRef.current?.focus()
  }

  const handleCreateTag = () => {
    if (canCreateNew) {
      createTagMutation.mutate({
        name: inputValue.trim(),
        color: selectedColor,
      })
    }
  }

  return (
    <div className="relative">
      {/* UI note */}
      {selectedTags.length > 0 && (
        <div className="mb-2 flex flex-wrap gap-1">
          {selectedTags.map((tag) => (
            <TagBadge key={tag.id} name={tag.name} color={tag.color} size="sm" onRemove={() => onRemoveTag(tag)} />
          ))}
        </div>
      )}

      {/* UI note */}
      <div className="relative">
        <Input
          ref={inputRef}
          type="text"
          role="combobox"
          aria-expanded={isOpen}
          aria-controls="tag-listbox"
          aria-activedescendant={highlightedIndex >= 0 ? `tag-option-${highlightedIndex}` : undefined}
          value={inputValue}
          onChange={handleInputChange}
          onKeyDown={handleKeyDown}
          onFocus={() => setIsOpen(true)}
          placeholder={placeholder ?? t('timeline.addTag')}
          inputSize="sm"
        />

        {/* UI note */}
        {isOpen && (inputValue || filteredTags.length > 0) && (
          <div
            ref={dropdownRef}
            id="tag-listbox"
            role="listbox"
            aria-label={t('timeline.tagSuggestions', 'Tag suggestions')}
            className={cn(
              'absolute mt-1 w-full rounded-lg',
              elevation.dropdown,
              'border border-muted bg-surface-overlay',
              'max-h-60 overflow-auto',
            )}
          >
            {/* UI note */}
            {filteredTags.length > 0 && (
              <div className="p-1">
                {filteredTags.map((tag, index) => (
                  <button
                    key={tag.id}
                    id={`tag-option-${index}`}
                    type="button"
                    role="option"
                    aria-selected={index === highlightedIndex}
                    className={cn(
                      'flex w-full items-center gap-2 rounded-md px-3 py-2 text-left',
                      `${motion.colors} hover:bg-hover`,
                      index === highlightedIndex && 'bg-hover',
                    )}
                    onClick={() => handleSelectTag(tag)}
                  >
                    <span className={`${iconSize.xs} rounded-full`} style={{ backgroundColor: tag.color }} />
                    <span className="text-content text-sm">{tag.name}</span>
                  </button>
                ))}
              </div>
            )}

            {/* UI note */}
            {canCreateNew && (
              <>
                {filteredTags.length > 0 && <Divider className="border-muted" />}
                <div className="p-2">
                  <div className="mb-2 px-1 text-content-secondary text-xs">{t('timeline.createNewTag')}</div>
                  <div className="mb-2 flex items-center gap-2">
                    <div className="flex gap-1">
                      {TAG_COLORS.slice(0, 5).map((color) => (
                        <button
                          key={color}
                          type="button"
                          className={cn(
                            `${iconSize.md} rounded-full ${motion.transform}`,
                            selectedColor === color && 'scale-110 ring-2 ring-slate-400 ring-offset-1',
                          )}
                          style={{ backgroundColor: color }}
                          onClick={() => setSelectedColor(color)}
                          aria-label={`${t('timeline.selectColor')} ${color}`}
                        />
                      ))}
                    </div>
                  </div>
                  <button
                    type="button"
                    className={cn(
                      'flex w-full items-center gap-2 rounded-md px-3 py-2 text-left',
                      `bg-brand-signal/10 ${motion.colors} hover:bg-brand-signal/20`,
                    )}
                    onClick={handleCreateTag}
                    disabled={createTagMutation.isPending}
                  >
                    <span className={`${iconSize.xs} rounded-full`} style={{ backgroundColor: selectedColor }} />
                    <span className="text-brand-text text-sm">
                      "{inputValue}" {t('timeline.createTag')}
                    </span>
                    {createTagMutation.isPending && (
                      <span className="ml-auto text-content-tertiary text-xs">{t('timeline.creating')}</span>
                    )}
                  </button>
                </div>
              </>
            )}

            {/* UI note */}
            {filteredTags.length === 0 && !canCreateNew && (
              <div className="p-3 text-center text-content-secondary text-sm">{t('timeline.noSearchResults')}</div>
            )}
          </div>
        )}
      </div>
    </div>
  )
}

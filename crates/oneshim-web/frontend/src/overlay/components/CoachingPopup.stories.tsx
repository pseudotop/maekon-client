import type { Meta, StoryObj } from '@storybook/react'
import type { CoachingPayload } from '../types'
import CoachingPopup from './CoachingPopup'

const meta = {
  title: 'Overlay/CoachingPopup',
  component: CoachingPopup,
  tags: ['autodocs'],
  parameters: {
    layout: 'fullscreen',
  },
} satisfies Meta<typeof CoachingPopup>

export default meta
type Story = StoryObj<typeof meta>

const defaultMessage: CoachingPayload = {
  message_id: 'msg-1',
  profile: 'default',
  trigger_type: 'idle_detected',
  text: 'You have been idle for a while. Consider taking a short walk or stretching before resuming work.',
  auto_dismiss_secs: 15,
  explanation: '',
}

/** Default coaching message with OK/Later buttons and feedback thumbs. */
export const Default: Story = {
  args: {
    message: defaultMessage,
    autoDismissSecs: 15,
  },
}

/** Long coaching text that wraps multiple lines. */
export const LongMessage: Story = {
  args: {
    message: {
      ...defaultMessage,
      message_id: 'msg-2',
      trigger_type: 'focus_drift',
      text: 'Your focus has shifted between 5 different applications in the last 10 minutes. Try closing non-essential windows and setting a clear goal for the next 25 minutes. The Pomodoro technique can help maintain deep focus.',
    },
    autoDismissSecs: 20,
  },
}

/** Short auto-dismiss timer. */
export const QuickDismiss: Story = {
  args: {
    message: {
      ...defaultMessage,
      message_id: 'msg-3',
      text: 'Great progress! Keep it up.',
    },
    autoDismissSecs: 5,
  },
}

/** Coaching message with a "Why?" explanation section. */
export const WithExplanation: Story = {
  args: {
    message: {
      ...defaultMessage,
      message_id: 'msg-4',
      trigger_type: 'focus_drift',
      text: 'Consider closing some browser tabs to reduce distractions.',
      explanation:
        'You switched between 8 browser tabs in the last 3 minutes, which is above your usual focus pattern. Reducing open tabs has been shown to improve deep work periods.',
    },
    autoDismissSecs: 20,
  },
}

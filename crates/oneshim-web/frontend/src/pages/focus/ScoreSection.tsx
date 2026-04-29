/**
 * Focus score section — circular gauge, trend stat cards, and weekly trend chart.
 *
 * Owns the Focus empty state (`focus_score === 0`) and error state so that
 * FocusLayout can always render <Outlet>, letting the `/focus` → `/focus/score`
 * index redirect fire even when metrics are empty or errored.
 */

import { Brain, Clock, MessageSquare, TrendingDown, TrendingUp, Zap } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts'
import StatCard from '../../components/StatCard'
import { EmptyState } from '../../components/ui'
import { Card, CardContent, CardHeader, CardTitle } from '../../components/ui/Card'
import { useTypedOutletContext } from '../../routes'
import { chart, dataViz, iconSize, motion, typography } from '../../styles/tokens'
import { formatDuration } from '../../utils/formatters'
import { scoreColor } from '../../utils/score-color'
import type { FocusContext } from './FocusLayout'

function CircularGauge({ value, size = 120 }: { value: number; size?: number }) {
  const { t } = useTranslation()
  const percentage = Math.min(value / 100, 1)
  const circumference = 2 * Math.PI * 45
  const strokeDashoffset = circumference * (1 - percentage)

  const color = scoreColor(value)

  return (
    <svg width={size} height={size} viewBox="0 0 100 100" aria-hidden="true">
      <circle cx="50" cy="50" r="45" fill="none" stroke="currentColor" strokeWidth="8" className="text-border-muted" />
      <circle
        cx="50"
        cy="50"
        r="45"
        fill="none"
        stroke={color}
        strokeWidth="8"
        strokeLinecap="round"
        strokeDasharray={circumference}
        strokeDashoffset={strokeDashoffset}
        transform="rotate(-90 50 50)"
        className={motion.all}
      />
      <text
        x="50"
        y="45"
        textAnchor="middle"
        dominantBaseline="middle"
        className={`fill-content ${typography.weight.bold} text-2xl`}
      >
        {Math.round(value)}
      </text>
      <text x="50" y="62" textAnchor="middle" className="fill-content-secondary text-xs">
        {t('focus.score')}
      </text>
    </svg>
  )
}

export default function ScoreSection() {
  const { t } = useTranslation()
  const { metrics, metricsError } = useTypedOutletContext<FocusContext>('Focus')

  if (metricsError || !metrics) {
    return (
      <Card variant="danger">
        <CardContent>
          <p className="text-semantic-error">{metricsError || t('common.error')}</p>
        </CardContent>
      </Card>
    )
  }

  if (metrics.today.focus_score === 0) {
    return (
      <EmptyState
        icon={<Brain className="h-8 w-8" />}
        title={t('emptyState.focus.title')}
        description={t('emptyState.focus.description')}
      />
    )
  }

  const today = metrics.today

  const historyData = [...metrics.history].reverse().map((m) => ({
    date: m.date.slice(5), // MM-DD
    score: m.focus_score,
    deepWork: Math.round(m.deep_work_secs / 60),
    communication: Math.round(m.communication_secs / 60),
  }))

  const avgScore =
    metrics.history.length > 0 ? metrics.history.reduce((a, b) => a + b.focus_score, 0) / metrics.history.length : 0
  const trend = today.focus_score - avgScore

  return (
    <>
      {/* Score cards */}
      <div id="section-score" className="grid grid-cols-2 gap-4 md:grid-cols-4">
        <Card variant="elevated" className="flex flex-col items-center justify-center p-6">
          <CircularGauge value={today.focus_score} />
          <div className="mt-2 flex items-center gap-1">
            {trend >= 0 ? (
              <TrendingUp className={`${iconSize.base} text-semantic-success`} />
            ) : (
              <TrendingDown className={`${iconSize.base} text-semantic-error`} />
            )}
            <span
              className={`${typography.weight.medium} text-sm ${trend >= 0 ? 'text-semantic-success' : 'text-semantic-error'}`}
            >
              {trend >= 0 ? '+' : ''}
              {trend.toFixed(1)}
            </span>
          </div>
        </Card>

        <StatCard
          title={t('focus.deepWork')}
          value={formatDuration(today.deep_work_secs, true)}
          icon={<Clock className={`${iconSize.md} text-brand-text`} />}
        />

        <StatCard
          title={t('focus.communication')}
          value={formatDuration(today.communication_secs, true)}
          icon={<MessageSquare className={`${iconSize.md} text-brand-text`} />}
        />

        <StatCard
          title={t('focus.interruptions')}
          value={`${today.interruption_count}${t('focus.times')}`}
          icon={<Zap className={`${iconSize.md} text-brand-text`} />}
        />
      </div>

      {/* Weekly trend chart */}
      <Card id="section-trend">
        <CardHeader>
          <CardTitle>{t('focus.weeklyTrend')}</CardTitle>
        </CardHeader>
        <CardContent>
          {historyData.length > 0 ? (
            <div className="h-64">
              <ResponsiveContainer width="100%" height="100%">
                <LineChart data={historyData}>
                  <CartesianGrid strokeDasharray="3 3" className="stroke-border-muted" />
                  <XAxis dataKey="date" className="text-xs" />
                  <YAxis domain={[0, 100]} className="text-xs" />
                  <Tooltip contentStyle={chart.tooltipStyle} labelStyle={chart.labelStyle} />
                  <Line
                    type="monotone"
                    dataKey="score"
                    name={t('focus.score')}
                    stroke={dataViz.stroke.good}
                    strokeWidth={2}
                    dot={{ fill: dataViz.stroke.good, r: 4 }}
                    activeDot={{ r: 6 }}
                  />
                </LineChart>
              </ResponsiveContainer>
            </div>
          ) : (
            <p className="py-8 text-center text-content-tertiary">{t('common.noData')}</p>
          )}
        </CardContent>
      </Card>
    </>
  )
}

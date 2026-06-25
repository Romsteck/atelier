import { memo } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeSanitize from 'rehype-sanitize';
import rehypeHighlight from 'rehype-highlight';

const REMARK = [remarkGfm];
const REHYPE = [rehypeSanitize, rehypeHighlight];

// Mémoïsé : le rendu markdown (parse + highlight) est coûteux. Dans le fil d'agent, la
// liste se re-render à chaque delta et à chaque frappe (avant l'extraction du Composer) ;
// sans memo, chaque message ré-parsait son markdown. Pur sur `children`/`className`.
function MarkdownView({ children, className = '' }) {
  return (
    <div
      className={`prose dark:prose-invert prose-sm max-w-none
                  prose-headings:text-gray-50 prose-headings:font-semibold
                  prose-p:text-gray-200
                  prose-li:text-gray-200
                  prose-strong:text-gray-50
                  prose-code:text-blue-700 dark:prose-code:text-blue-300 prose-code:bg-gray-800/60 prose-code:px-1 prose-code:py-0.5 prose-code:rounded-sm prose-code:before:content-none prose-code:after:content-none
                  prose-pre:bg-gray-900 prose-pre:border prose-pre:border-gray-700 prose-pre:text-gray-800 dark:prose-pre:text-gray-100
                  prose-a:text-blue-600 dark:prose-a:text-blue-400 prose-a:no-underline prose-a:hover:underline
                  prose-blockquote:border-l-blue-500 prose-blockquote:text-gray-300
                  prose-table:text-sm
                  ${className}`}
    >
      <ReactMarkdown remarkPlugins={REMARK} rehypePlugins={REHYPE}>
        {children || ''}
      </ReactMarkdown>
    </div>
  );
}

export default memo(MarkdownView);

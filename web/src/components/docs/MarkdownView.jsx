import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import rehypeSanitize from 'rehype-sanitize';
import rehypeHighlight from 'rehype-highlight';

const REMARK = [remarkGfm];
const REHYPE = [rehypeSanitize, rehypeHighlight];

export default function MarkdownView({ children, className = '' }) {
  return (
    <div
      className={`prose prose-invert prose-sm max-w-none
                  prose-headings:text-white prose-headings:font-semibold
                  prose-p:text-gray-200
                  prose-li:text-gray-200
                  prose-strong:text-white
                  prose-code:text-blue-300 prose-code:bg-gray-800/60 prose-code:px-1 prose-code:py-0.5 prose-code:rounded prose-code:before:content-none prose-code:after:content-none
                  prose-pre:bg-gray-900 prose-pre:border prose-pre:border-gray-700
                  prose-a:text-blue-400 prose-a:no-underline hover:prose-a:underline
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

import { useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { User, LogOut, Lock, CheckCircle, AlertCircle } from 'lucide-react';
import { useAuth } from '../context/AuthContext';
import { changeCode } from '../api/client';
import PageHeader from '../components/PageHeader';

function Profile() {
  const { user, logout } = useAuth();
  const navigate = useNavigate();
  const [newCode, setNewCode] = useState('');
  const [codeSuccess, setCodeSuccess] = useState('');
  const [codeError, setCodeError] = useState('');

  const handleLogout = async () => {
    await logout();
    navigate('/login');
  };

  const handleChangeCode = async (e) => {
    e.preventDefault();
    setCodeSuccess('');
    setCodeError('');

    try {
      const res = await changeCode(newCode);
      if (res.data.success) {
        setCodeSuccess('Code mis a jour avec succes');
        setNewCode('');
      } else {
        setCodeError(res.data.error || 'Erreur lors du changement de code');
      }
    } catch (err) {
      setCodeError(err.response?.data?.error || err.message || 'Erreur lors du changement de code');
    }
  };

  if (!user) {
    return null;
  }

  return (
    <div>
      <PageHeader title="Mon compte" icon={User}>
        <button
          onClick={handleLogout}
          className="flex items-center gap-2 px-4 py-2 bg-gray-800/50 hover:bg-gray-700/50 border border-gray-700 text-gray-300 transition-colors"
        >
          <LogOut className="w-4 h-4" />
          <span className="hidden sm:inline">Deconnexion</span>
        </button>
      </PageHeader>

      <div className="space-y-6">
        {/* User Info Card */}
        <div className="bg-gray-800/50 backdrop-blur-sm p-6 border border-gray-700">
          <h2 className="text-lg font-semibold text-white mb-4">Informations du compte</h2>

          <div className="space-y-px">
            <div className="flex items-center gap-3">
              <div className="w-10 h-10 bg-gray-700/50  flex items-center justify-center">
                <User className="w-5 h-5 text-gray-400" />
              </div>
              <div>
                <div className="text-sm text-gray-400">Nom d'utilisateur</div>
                <div className="text-white font-medium">{user.username}</div>
              </div>
            </div>

            <div className="flex items-center gap-3">
              <div className="w-10 h-10 bg-gray-700/50  flex items-center justify-center">
                <span className="text-lg">{(user.displayName || user.username)?.charAt(0)?.toUpperCase()}</span>
              </div>
              <div>
                <div className="text-sm text-gray-400">Nom d'affichage</div>
                <div className="text-white font-medium">{user.displayName || user.username}</div>
              </div>
            </div>
          </div>
        </div>

        {/* Change Code Card */}
        <div className="bg-gray-800/50 backdrop-blur-sm p-6 border border-gray-700">
          <h2 className="text-lg font-semibold text-white mb-4">Changer le code d'acces</h2>

          <form onSubmit={handleChangeCode} className="space-y-4">
            {codeSuccess && (
              <div className="flex items-center gap-2 p-3 bg-green-500/20 border border-green-500/50 text-green-400">
                <CheckCircle className="w-5 h-5 flex-shrink-0" />
                <span className="text-sm">{codeSuccess}</span>
              </div>
            )}

            {codeError && (
              <div className="flex items-center gap-2 p-3 bg-red-500/20 border border-red-500/50 text-red-400">
                <AlertCircle className="w-5 h-5 flex-shrink-0" />
                <span className="text-sm">{codeError}</span>
              </div>
            )}

            <div>
              <label className="block text-sm font-medium text-gray-300 mb-2">
                Nouveau code
              </label>
              <div className="relative">
                <Lock className="absolute left-3 top-1/2 -translate-y-1/2 w-5 h-5 text-gray-500" />
                <input
                  type="password"
                  value={newCode}
                  onChange={(e) => setNewCode(e.target.value)}
                  className="w-full pl-10 pr-4 py-3 bg-gray-900/50 border border-gray-600 text-white placeholder-gray-500 focus:border-blue-500 focus:ring-2 focus:ring-blue-500/20 transition-all"
                  placeholder="Nouveau code d'acces"
                  required
                />
              </div>
            </div>

            <button
              type="submit"
              className="px-4 py-2 bg-blue-600 hover:bg-blue-700 text-white font-medium transition-colors"
            >
              Mettre a jour
            </button>
          </form>
        </div>
      </div>
    </div>
  );
}

export default Profile;
